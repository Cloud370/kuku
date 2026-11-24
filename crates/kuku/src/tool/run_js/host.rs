use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, Semaphore};

use crate::tool::builtin::common::ExecutorObservation;
use crate::tool::{ToolDefinition, ToolResultEnvelope};

use super::{
    ScriptOperation, WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptLimits,
};

const MAX_ACTION_FACT_BYTES: usize = 8192;
const MAX_WRITTEN_PREVIEW_BYTES: usize = 1024;
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
    fn retain_return(
        &self,
        _json: String,
    ) -> ScriptFuture<'_, crate::Result<Option<crate::OutputReference>>> {
        Box::pin(async { Ok(None) })
    }
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
    stopped: OnceLock<StopReason>,
    cancelling: Arc<Notify>,
    host_operations: Arc<Semaphore>,
    facts: Mutex<BTreeMap<usize, WorkspaceScriptActionFacts>>,
    staged_observations: Mutex<BTreeMap<String, ExecutorObservation>>,
    observations: Mutex<Vec<ExecutorObservation>>,
    pub(super) outputs: super::outputs::DerivedOutputs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Stopped,
    NewInput,
}

#[derive(Default)]
pub(super) struct ActionCounts {
    pub(super) dispatched: usize,
    pub(super) completed: usize,
    pub(super) cancelled: usize,
}

impl ScriptExecution {
    pub(crate) fn cancellation(&self) -> Arc<Notify> {
        self.cancelling.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.stopped.get().is_some()
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
            stopped: OnceLock::new(),
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
        self.stop_with_reason(StopReason::Stopped);
    }

    pub(crate) fn stop_for_new_input(&self) {
        self.stop_with_reason(StopReason::NewInput);
    }

    pub(super) fn stopped_for_new_input(&self) -> bool {
        self.stopped.get() == Some(&StopReason::NewInput)
    }

    fn stop_with_reason(&self, reason: StopReason) {
        if self.stopped.set(reason).is_ok() {
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

    pub(super) fn facts(&self) -> (Vec<WorkspaceScriptActionFacts>, ActionCounts) {
        let facts = self.facts.lock().unwrap();
        let mut counts = ActionCounts {
            dispatched: facts.len(),
            ..Default::default()
        };
        let mut combined: Vec<WorkspaceScriptActionFacts> = Vec::with_capacity(facts.len());
        for fact in facts.values() {
            counts.completed += usize::from(fact.status == WorkspaceScriptActionStatus::Ok);
            counts.cancelled += usize::from(fact.status == WorkspaceScriptActionStatus::Cancelled);
            if combined
                .last_mut()
                .is_some_and(|last| merge_output_read(last, fact))
            {
                continue;
            }
            combined.push(fact.clone());
        }
        (combined, counts)
    }

    async fn dispatch(
        self: &Arc<Self>,
        host: &dyn ScriptHost,
        ordinal: usize,
        op: ScriptOperation,
        args: Value,
    ) -> ToolResultEnvelope {
        use WorkspaceScriptActionStatus::{Blocked, Cancelled, Error, Ok, Unknown};

        let written = (op == ScriptOperation::WriteFile)
            .then(|| args["content"].as_str())
            .flatten()
            .map(|text| {
                (
                    text.len(),
                    cap_str(text, MAX_WRITTEN_PREVIEW_BYTES).to_owned(),
                )
            });
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
        fact.result = bounded_fact(result.structured.as_ref(), written);
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
    crate::util::json::write_bounded_json(value, limit, Vec::new())
}

pub(super) fn cap_str(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn not_dispatched() -> ToolResultEnvelope {
    ToolResultEnvelope::cancelled("run_js stopped before dispatch")
}

fn action_id(ordinal: usize) -> String {
    format!("action_{ordinal}")
}

fn observation_bytes(read: &ExecutorObservation) -> usize {
    std::mem::size_of::<ExecutorObservation>()
        + read.canonical_path.as_os_str().len()
        + read.content_hash.len()
}

fn bounded_fact(structured: Option<&Value>, written: Option<(usize, String)>) -> Value {
    let Some(structured) = structured else {
        return Value::Null;
    };
    let json = match structured {
        Value::Object(fields) => {
            let preview = written
                .filter(|(bytes, _)| {
                    structured["kind"] == "file_write"
                        && structured["bytes_written"].as_u64() == Some(*bytes as u64)
                })
                .map(|(bytes, prefix)| {
                    let truncated = prefix.len() < bytes;
                    (Value::String(prefix), Value::Bool(truncated))
                });
            let mut retained: BTreeMap<_, _> = fields
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "raw_text"
                            | "raw_text_after"
                            | "model_content"
                            | "segments"
                            | "inline_pipes"
                            | "content"
                            | "text"
                    )
                })
                .map(|(key, value)| (key.as_str(), value))
                .collect();
            if let Some((text, truncated)) = &preview {
                retained.insert("raw_text_after", text);
                retained.insert("raw_text_after_truncated", truncated);
            }
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
        "started",
        "signal",
        "capture_complete",
        "cleanup_incomplete",
        "output_id",
        "output_ref",
        "name",
        "reference",
        "change",
        "path",
        "canonical_path",
        "commit_state",
        "content_hash",
        "content_hash_before",
        "content_hash_after",
        "bytes_written",
        "bytes_removed",
        "created",
        "line_count",
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
    if facts.get("change").is_none() {
        let change = &structured["change"];
        let mut summary = serde_json::Map::new();
        for field in ["added_lines", "removed_lines"] {
            if let Some(value) = change[field].as_u64() {
                summary.insert(field.to_owned(), value.into());
            }
        }
        if let Some(line) = change["first_changed_line"].as_u64().or_else(|| {
            change["preview_line_numbers"]
                .as_array()?
                .iter()
                .filter_map(Value::as_u64)
                .find(|line| *line > 0)
        }) {
            summary.insert("first_changed_line".into(), line.into());
        }
        if !summary.is_empty() {
            facts["change"] = Value::Object(summary);
        }
    }
    if structured["termination"].as_str().is_some() {
        if let Some(program) = super::recovery::command_program(structured) {
            facts["program"] = json!(program);
        }
    }
    facts
}

pub(super) fn bound_action_facts(actions: &mut [WorkspaceScriptActionFacts]) {
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

#[cfg(test)]
mod tests {
    #[test]
    fn reduced_action_facts_keep_the_first_changed_line() {
        let outline = super::selected_facts(&serde_json::json!({
            "kind": "file_edit",
            "change": {"preview": "+".to_owned() + &"a".repeat(700), "added_lines": 1,
                       "removed_lines": 0, "preview_line_numbers": [17]}
        }));
        assert_eq!(outline["change"]["first_changed_line"], 17);
        assert_eq!(outline["change"]["added_lines"], 1);
        assert!(outline["change"].get("preview").is_none());
    }
}
