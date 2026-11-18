use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{mpsc as thread_channel, Arc};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Notify, Semaphore};
use tokio::task::JoinSet;

use crate::session::runtime::WorkspaceWorkerPermit;
use crate::tool::builtin::common::ExecutorRead;
use crate::tool::{ToolFaults, ToolResultEnvelope};
use crate::WorkspaceExecutionHost;

use super::host::{bounded_json, cap_str};
use super::vm::{self, BufferedResponse, Reply, Request};
use super::{
    ScriptExecution, ScriptHost, ScriptOperation, WorkspaceScriptActionStatus,
    WorkspaceScriptFacts, WorkspaceScriptLimits, WorkspaceScriptResultKind,
};

const HOST_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const NATIVE_STACK_HEADROOM: usize = 2 * 1024 * 1024;
const MAX_HOST_MESSAGE_BYTES: usize = 2048;

pub(crate) async fn execute(
    code: String,
    host: Arc<dyn ScriptHost>,
    limits: WorkspaceScriptLimits,
    host_capacity: WorkspaceExecutionHost,
    cancellation: Arc<Notify>,
) -> ToolResultEnvelope {
    if let Err(error) = limits.validate() {
        return ToolResultEnvelope::error("invalid workspace script limits", error);
    }
    if code.trim().is_empty() || code.len() > limits.source_bytes {
        return ToolResultEnvelope::error(
            "invalid workspace script",
            format!("code must contain 1–{} UTF-8 bytes", limits.source_bytes),
        );
    }
    let Some(deadline) = Instant::now().checked_add(Duration::from_millis(limits.timeout_ms))
    else {
        return ToolResultEnvelope::error(
            "invalid workspace script time limit",
            "time limit exceeds the supported clock range",
        );
    };
    let cancelling = cancellation.notified();
    tokio::pin!(cancelling);
    let execution = Arc::new(ScriptExecution::new(limits, host_capacity.operations()));
    let admission = tokio::select! {
        biased;
        _ = &mut cancelling => return result(Finish::Cancelled, &execution),
        _ = sleep_until(deadline) => return result(Finish::TimedOut, &execution),
        permit = host_capacity.acquire_worker() => permit,
    };
    let permit = match admission {
        Ok(permit) => permit,
        Err(error) => return result(Finish::Blocked(error.to_string()), &execution),
    };
    let _stop_on_drop = StopOnDrop(execution.clone());
    let worker = match Worker::start(code, host.as_ref(), execution.clone(), deadline, permit) {
        Ok(worker) => worker,
        Err(error) => {
            return result(
                Finish::Failed(format!("could not start JavaScript worker: {error}")),
                &execution,
            )
        }
    };
    let finish = worker
        .drive(host, execution.clone(), deadline, cancelling)
        .await;
    result(finish, &execution)
}

struct Worker {
    requests: mpsc::Receiver<Request>,
    replies: thread_channel::Sender<Reply>,
    finished: oneshot::Receiver<Result<Value, vm::Failure>>,
}

impl Worker {
    fn start(
        code: String,
        host: &dyn ScriptHost,
        execution: Arc<ScriptExecution>,
        deadline: Instant,
        permit: WorkspaceWorkerPermit,
    ) -> std::io::Result<Self> {
        let (request_tx, requests) = mpsc::channel(execution.limits.queued_operations);
        let (replies, reply_rx) = thread_channel::channel();
        let (finish_tx, finished) = oneshot::channel();
        let native_stack = NATIVE_STACK_HEADROOM + execution.limits.stack_bytes;
        let methods = host.methods();
        let _worker = thread::Builder::new()
            .name("kuku-workspace-script".into())
            .stack_size(native_stack)
            .spawn(move || {
                let outcome = vm::run(code, methods, execution, deadline, request_tx, reply_rx);
                drop(permit);
                let _ = finish_tx.send(outcome);
            })?;
        Ok(Self {
            requests,
            replies,
            finished,
        })
    }

    async fn drive(
        mut self,
        host: Arc<dyn ScriptHost>,
        execution: Arc<ScriptExecution>,
        deadline: Instant,
        mut cancellation: Pin<&mut impl Future<Output = ()>>,
    ) -> Finish {
        let response_capacity = Arc::new(Semaphore::new(execution.limits.host_bytes));
        let mut actions = JoinSet::new();
        let mut requests_open = true;
        let mut worker_finished = false;
        let finish = loop {
            tokio::select! {
                biased;
                _ = &mut cancellation => break Finish::Cancelled,
                _ = sleep_until(deadline) => break Finish::TimedOut,
                _ = execution.cancelled() => break Finish::HostStopped,
                outcome = &mut self.finished => {
                    worker_finished = true;
                    break vm_finish(outcome);
                }
                Some(reply) = actions.join_next(), if !actions.is_empty() => {
                    match reply {
                        Ok(reply) => {
                            let _ = self.replies.send(reply);
                        }
                        Err(error) => {
                            execution.cleanup_incomplete.store(true, Ordering::Release);
                            break Finish::Failed(format!("workspace host operation failed: {error}"));
                        }
                    }
                }
                request = self.requests.recv(), if requests_open && actions.len() < execution.limits.active_operations => {
                    if let Some(request) = request {
                        actions.spawn(dispatch(request, host.clone(), execution.clone(), response_capacity.clone()));
                    } else {
                        requests_open = false;
                    }
                }
            }
        };
        execution.stop();
        drop(self.requests);
        drop(self.replies);
        let cleanup = async {
            if !worker_finished {
                let _ = (&mut self.finished).await;
            }
            while let Some(outcome) = actions.join_next().await {
                if outcome.is_err() {
                    execution.cleanup_incomplete.store(true, Ordering::Release);
                }
            }
        };
        if tokio::time::timeout(HOST_CLEANUP_TIMEOUT, cleanup)
            .await
            .is_err()
        {
            execution.cleanup_incomplete.store(true, Ordering::Release);
            actions.abort_all();
        }
        finish
    }
}

#[derive(Serialize)]
struct HostReply<'a> {
    status: &'a str,
    summary: &'a str,
    truncated: bool,
    result: Option<&'a Value>,
    model_content: &'a str,
}

enum Finish {
    Returned(Value),
    Failed(String),
    Blocked(String),
    Cancelled,
    TimedOut,
    HostStopped,
}

struct StopOnDrop(Arc<ScriptExecution>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop();
    }
}

async fn sleep_until(deadline: Instant) {
    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
}

async fn dispatch(
    request: Request,
    host: Arc<dyn ScriptHost>,
    execution: Arc<ScriptExecution>,
    response_capacity: Arc<Semaphore>,
) -> Reply {
    let args = serde_json::from_slice::<Value>(&request.json);
    drop(request.json);
    let response = match args {
        Ok(args) if args.is_object() => {
            let result = execution
                .call(host.as_ref(), request.ordinal, request.op, args)
                .await;
            let read = if request.op == ScriptOperation::ReadFile {
                execution.take_read(request.ordinal)
            } else {
                None
            };
            buffer_response(result, request.op, read, &execution, response_capacity).await
        }
        Ok(_) | Err(_) => Err("workspace host request must be a JSON object within nesting bounds"),
    };
    Reply {
        ordinal: request.ordinal,
        response,
    }
}

async fn buffer_response(
    envelope: ToolResultEnvelope,
    op: ScriptOperation,
    read: Option<ExecutorRead>,
    execution: &ScriptExecution,
    capacity: Arc<Semaphore>,
) -> Result<BufferedResponse, &'static str> {
    if execution.is_cancelled() {
        return Err("workspace script has stopped");
    }
    let read = read.filter(|_| envelope.status == "ok");
    let model_content = if envelope.status == "ok"
        && matches!(op, ScriptOperation::ReadFile | ScriptOperation::ReadOutput)
    {
        ""
    } else {
        envelope.model_content.as_str()
    };
    let mut response = HostReply {
        status: &envelope.status,
        summary: &envelope.summary,
        truncated: envelope.truncated,
        result: envelope.structured.as_ref(),
        model_content,
    };
    let limit = execution.limits.host_bytes;
    let json = match bounded_json(&response, limit) {
        Ok(json) => json,
        Err(_) => {
            response.model_content = cap_str(model_content, MAX_HOST_MESSAGE_BYTES);
            response.summary = cap_str(response.summary, MAX_HOST_MESSAGE_BYTES);
            response.truncated = true;
            bounded_json(&response, limit).map_err(|_| {
                "workspace host response exceeds its byte limit; read a smaller range or page"
            })?
        }
    };
    drop(envelope);
    let bytes = u32::try_from(json.len()).expect("validated host reply byte limit");
    let _capacity = tokio::select! {
        biased;
        _ = execution.cancelled() => return Err("workspace script has stopped"),
        permit = capacity.acquire_many_owned(bytes) => permit.map_err(|_| "workspace response queue is closed")?,
    };
    Ok(BufferedResponse {
        json,
        read,
        _capacity,
    })
}

fn vm_finish(result: Result<Result<Value, vm::Failure>, oneshot::error::RecvError>) -> Finish {
    match result {
        Ok(Ok(value)) => Finish::Returned(value),
        Ok(Err(vm::Failure::Error(error))) => Finish::Failed(error),
        Ok(Err(vm::Failure::TimedOut)) => Finish::TimedOut,
        Ok(Err(vm::Failure::Stopped)) => Finish::HostStopped,
        Err(_) => Finish::Failed("JavaScript worker exited before returning a result".into()),
    }
}

fn result(finish: Finish, execution: &ScriptExecution) -> ToolResultEnvelope {
    let (actions, dispatched) = execution.facts();
    let failures = actions
        .iter()
        .filter(|action| {
            action.status != WorkspaceScriptActionStatus::Ok
                || action.result["exit_code"]
                    .as_i64()
                    .is_some_and(|code| code != 0)
        })
        .count();
    let faults = ToolFaults {
        output_persistence_failed: execution.failed_persistence.load(Ordering::Acquire),
        cleanup_incomplete: execution.cleanup_incomplete.load(Ordering::Acquire),
    };
    let timed_out = matches!(finish, Finish::TimedOut);
    let outcome = match finish {
        Finish::Returned(value) => {
            let model_content = serde_json::to_string(&value).expect("script result is JSON");
            ToolResultEnvelope::ok(
                format!("workspace script returned after {dispatched} host actions ({failures} unsuccessful)"),
                model_content,
                value,
            )
        }
        Finish::Failed(error) => ToolResultEnvelope::error("workspace script failed", error),
        Finish::Blocked(error) => {
            ToolResultEnvelope::blocked("workspace script capacity unavailable", error)
        }
        Finish::Cancelled | Finish::HostStopped => {
            ToolResultEnvelope::cancelled("workspace script stopped")
        }
        Finish::TimedOut => ToolResultEnvelope::error(
            "workspace script timed out",
            format!(
                "workspace script exceeded its {} ms time limit",
                execution.limits.timeout_ms
            ),
        ),
    };
    let mut result = outcome;
    let value = if result.status == "ok" {
        result.structured.take().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    if faults.output_persistence_failed {
        result.status = "error".into();
        result.summary = "workspace script output persistence failed".into();
        result.model_content = "A host action could not persist its output.".into();
    }
    if faults.cleanup_incomplete {
        result.status = "error".into();
        result.summary = "workspace script cleanup incomplete".into();
        result
            .model_content
            .push_str("\nWorkspace script cleanup could not be confirmed.");
    }
    if result.status != "ok" && dispatched != 0 {
        result.model_content.push_str(&format!("\nHost action results observed: {dispatched} requested actions ({failures} unsuccessful)."));
    }
    let facts = WorkspaceScriptFacts {
        kind: WorkspaceScriptResultKind::WorkspaceScriptResult,
        value,
        actions,
        timed_out,
        cleanup_incomplete: faults.cleanup_incomplete,
        not_dispatched: execution
            .accepted
            .load(Ordering::Acquire)
            .saturating_sub(dispatched),
    };
    result.faults = faults;
    result.structured = Some(serde_json::to_value(facts).expect("script facts serialize to JSON"));
    result
}
