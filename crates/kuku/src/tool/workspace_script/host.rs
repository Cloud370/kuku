use std::collections::BTreeMap;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{Notify, Semaphore};

use crate::tool::builtin::common::ExecutorObservation;
use crate::tool::{ToolDefinition, ToolResultEnvelope};

use super::{
    ScriptOperation, WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptLimits,
};

const MAX_ACTION_FACT_BYTES: usize = 8192;
const MAX_ACTION_SUMMARY_BYTES: usize = 512;
const MAX_SCRIPT_FACT_BYTES: usize = 640 * 1024;
const MIN_ACTION_RECORD_BYTES: usize = 256;

pub(crate) type ScriptFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone)]
pub(crate) struct ScriptCall {
    pub(crate) id: String,
    pub(crate) op: ScriptOperation,
    pub(crate) args: Value,
}

pub(crate) trait ScriptHost: Send + Sync {
    fn methods(&self) -> Vec<ToolDefinition>;
    fn dispatch(
        &self,
        call: ScriptCall,
        execution: Arc<ScriptExecution>,
    ) -> ScriptFuture<'_, ToolResultEnvelope>;
}

pub(crate) struct ScriptExecution {
    pub(super) limits: WorkspaceScriptLimits,
    pub(super) failed_persistence: AtomicBool,
    pub(super) cleanup_incomplete: AtomicBool,
    pub(super) accepted: AtomicUsize,
    stopped: AtomicBool,
    cancelling: Arc<Notify>,
    host_operations: Arc<Semaphore>,
    facts: Mutex<BTreeMap<usize, WorkspaceScriptActionFacts>>,
    staged_observations: Mutex<BTreeMap<String, ExecutorObservation>>,
    observations: Mutex<Vec<ExecutorObservation>>,
    pub(super) outputs: super::outputs::DerivedOutputs,
}

impl ScriptExecution {
    pub(crate) fn cancellation(&self) -> Arc<Notify> {
        self.cancelling.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        let wake = self.cancelling.notified();
        if !self.is_cancelled() {
            wake.await;
        }
    }

    pub(crate) fn new(limits: WorkspaceScriptLimits, host_operations: Arc<Semaphore>) -> Self {
        Self {
            outputs: super::outputs::DerivedOutputs::new(limits.clone()),
            limits,
            stopped: AtomicBool::new(false),
            failed_persistence: AtomicBool::new(false),
            cleanup_incomplete: AtomicBool::new(false),
            accepted: AtomicUsize::new(0),
            cancelling: Arc::new(Notify::new()),
            host_operations,
            facts: Mutex::new(BTreeMap::new()),
            staged_observations: Mutex::new(BTreeMap::new()),
            observations: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            self.cancelling.notify_waiters();
            self.cancelling.notify_one();
        }
    }

    pub(super) async fn call(
        self: &Arc<Self>,
        host: &dyn ScriptHost,
        ordinal: usize,
        op: ScriptOperation,
        args: Value,
    ) -> ToolResultEnvelope {
        let permit = tokio::select! {
            biased;
            _ = self.cancelled() => return not_dispatched(),
            permit = self.host_operations.acquire() => permit,
        };
        let Ok(_permit) = permit else {
            return not_dispatched();
        };
        if self.is_cancelled() {
            return not_dispatched();
        }
        self.dispatch(host, ordinal, op, args).await
    }

    pub(crate) fn remember_observation(&self, observation: ExecutorObservation) {
        let mut observations = self.observations.lock().unwrap();
        if observations.contains(&observation) {
            return;
        }
        observations.push(observation);
        let mut bytes = observations.iter().map(observation_bytes).sum::<usize>();
        let discard = observations
            .iter()
            .take_while(|read| {
                let full = bytes > self.limits.host_bytes;
                bytes -= observation_bytes(read);
                full
            })
            .count();
        observations.drain(..discard);
    }

    pub(crate) fn observations(&self) -> Vec<ExecutorObservation> {
        self.observations.lock().unwrap().clone()
    }

    pub(crate) fn stage_observation(&self, id: &str, observation: ExecutorObservation) {
        self.staged_observations
            .lock()
            .unwrap()
            .insert(id.into(), observation);
    }

    pub(super) fn take_observation(&self, ordinal: usize) -> Option<ExecutorObservation> {
        self.staged_observations
            .lock()
            .unwrap()
            .remove(&action_id(ordinal))
    }

    pub(super) fn facts(&self) -> (Vec<WorkspaceScriptActionFacts>, usize) {
        let facts = self.facts.lock().unwrap();
        let mut combined: Vec<WorkspaceScriptActionFacts> = Vec::with_capacity(facts.len());
        for fact in facts.values() {
            if combined
                .last_mut()
                .is_some_and(|last| merge_output_read(last, fact))
            {
                continue;
            }
            combined.push(fact.clone());
        }
        let dispatched = facts.len();
        drop(facts);
        bound_action_facts(&mut combined);
        (combined, dispatched)
    }

    async fn dispatch(
        self: &Arc<Self>,
        host: &dyn ScriptHost,
        ordinal: usize,
        op: ScriptOperation,
        args: Value,
    ) -> ToolResultEnvelope {
        use WorkspaceScriptActionStatus::{Blocked, Cancelled, Error, Ok, Unknown};

        let call = ScriptCall {
            id: action_id(ordinal),
            op,
            args,
        };
        let mut fact = WorkspaceScriptActionFacts {
            id: call.id.clone(),
            tool: call.op,
            status: Unknown,
            summary: "host action outcome is unconfirmed".into(),
            truncated: false,
            result: Value::Null,
        };
        self.facts.lock().unwrap().insert(ordinal, fact.clone());
        let result = host.dispatch(call, self.clone()).await;
        fact.status = match result.status.as_str() {
            "ok" => Ok,
            "error" => Error,
            "blocked" => Blocked,
            "cancelled" => Cancelled,
            _ => Unknown,
        };
        fact.summary = cap_str(&result.summary, MAX_ACTION_SUMMARY_BYTES).into();
        fact.truncated = result.truncated;
        fact.result = bounded_fact(result.structured.as_ref());
        self.facts.lock().unwrap().insert(ordinal, fact);
        self.failed_persistence
            .fetch_or(result.faults.output_persistence_failed, Ordering::AcqRel);
        self.cleanup_incomplete
            .fetch_or(result.faults.cleanup_incomplete, Ordering::AcqRel);
        if result.faults.output_persistence_failed {
            self.stop();
        }
        result
    }
}

pub(super) fn bounded_json(value: &impl Serialize, limit: usize) -> serde_json::Result<Vec<u8>> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.bytes)
}

pub(super) fn cap_str(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn not_dispatched() -> ToolResultEnvelope {
    ToolResultEnvelope::cancelled("workspace script stopped before dispatch")
}

fn action_id(ordinal: usize) -> String {
    format!("action_{ordinal}")
}

fn observation_bytes(read: &ExecutorObservation) -> usize {
    std::mem::size_of::<ExecutorObservation>()
        + read.canonical_path.as_os_str().len()
        + read.content_hash.len()
}

fn bounded_fact(structured: Option<&Value>) -> Value {
    let Some(structured) = structured else {
        return Value::Null;
    };
    let json = match structured {
        Value::Object(fields) => {
            let retained: BTreeMap<_, _> = fields
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "raw_text"
                            | "raw_text_after"
                            | "model_content"
                            | "segments"
                            | "content"
                            | "text"
                    )
                })
                .collect();
            bounded_json(&retained, MAX_ACTION_FACT_BYTES)
        }
        value => bounded_json(value, MAX_ACTION_FACT_BYTES),
    };
    if let Ok(bytes) = json {
        if let Ok(fact) = serde_json::from_slice(&bytes) {
            return fact;
        }
    }
    selected_facts(structured)
}

fn selected_facts(structured: &Value) -> Value {
    let mut facts = json!({ "details_omitted": true });
    for key in [
        "kind",
        "exit_code",
        "termination",
        "capture_complete",
        "cleanup_incomplete",
        "output_id",
        "output_ref",
        "name",
        "reference",
        "change",
        "path",
        "content_hash",
        "offset",
        "next_offset",
        "pages",
        "complete",
        "capture_truncated",
    ] {
        if let Some(value) = structured.get(key) {
            if bounded_json(value, MAX_ACTION_SUMMARY_BYTES).is_ok() {
                facts[key] = value.clone();
            }
        }
    }
    facts
}

fn bound_action_facts(actions: &mut [WorkspaceScriptActionFacts]) {
    let mut available_bytes = MAX_SCRIPT_FACT_BYTES - actions.len() * MIN_ACTION_RECORD_BYTES;
    for action in actions {
        let limit = available_bytes + MIN_ACTION_RECORD_BYTES;
        let bytes = bounded_json(action, limit).unwrap_or_else(|_| {
            action.summary = "action details exceeded script result budget".into();
            action.result = selected_facts(&action.result);
            bounded_json(action, limit).unwrap_or_else(|_| {
                action.result = json!({"details_omitted":true});
                bounded_json(action, limit).expect("compact action fits its fixed budget")
            })
        });
        available_bytes = limit - bytes.len();
    }
}

fn merge_output_read(
    left: &mut WorkspaceScriptActionFacts,
    right: &WorkspaceScriptActionFacts,
) -> bool {
    if left.tool != ScriptOperation::ReadOutput
        || right.tool != ScriptOperation::ReadOutput
        || left.status != WorkspaceScriptActionStatus::Ok
        || right.status != WorkspaceScriptActionStatus::Ok
    {
        return false;
    }
    let Some(output_id) = left.result["output_id"].as_str() else {
        return false;
    };
    if right.result["output_id"] != output_id
        || left.result["next_offset"].as_u64().is_none()
        || left.result["next_offset"] != right.result["offset"]
    {
        return false;
    }
    let pages = left.result["pages"].as_u64().unwrap_or(1) + 1;
    left.summary = format!("read {pages} pages of output {output_id}");
    left.truncated |= right.truncated;
    left.result["pages"] = pages.into();
    for key in [
        "next_offset",
        "total_bytes",
        "has_more",
        "complete",
        "capture_truncated",
        "cleanup_incomplete",
    ] {
        left.result[key] = right.result[key].clone();
    }
    for key in ["lossy_utf8", "filtered"] {
        left.result[key] = (left.result[key].as_bool().unwrap_or(false)
            || right.result[key].as_bool().unwrap_or(false))
        .into();
    }
    true
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized JSON exceeds byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
