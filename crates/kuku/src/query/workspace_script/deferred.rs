use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;

use super::{prepare_file_target, WorkspaceHost};
use crate::event::{EventPayload, EventStore};
use crate::permission::GateDecisionKind;
use crate::query::helpers::{display_summary, now_timestamp, permission_candidate};
use crate::query::types::{PendingRun, PermissionRequest, QueuedToolCall};
use crate::tool::builtin::common::ExecutorObservation;
use crate::tool::workspace_script::{ScriptCall, ScriptExecution, ScriptOperation};
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, Result, WorkspaceScriptActionStatus, WorkspaceScriptFacts};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct PreparedAction {
    id: String,
    operation: ScriptOperation,
    args: Value,
    #[serde(rename = "reads")]
    observations: Vec<ExecutorObservation>,
    workspace: PathBuf,
    definition: ToolDefinition,
    candidate: String,
}

#[derive(Debug)]
pub(crate) struct AwaitingAction {
    pub(in crate::query) queued: QueuedToolCall,
    pub(crate) request: PermissionRequest,
    pub(crate) partial: ToolResultEnvelope,
    continuation: Continuation,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Continuation {
    action: PreparedAction,
    facts: WorkspaceScriptFacts,
}

impl PreparedAction {
    pub(super) fn new(
        call: &ScriptCall,
        args: Value,
        definition: &ToolDefinition,
        candidate: String,
        workspace: &Path,
        execution: &ScriptExecution,
    ) -> Result<Self> {
        let observations = match call.op {
            ScriptOperation::EditFile
            | ScriptOperation::WriteFile
            | ScriptOperation::RemoveFile => {
                let path = workspace.join(args["path"].as_str().unwrap_or_default());
                execution
                    .observations()
                    .into_iter()
                    .filter(|read| read.canonical_path == path)
                    .collect()
            }
            _ => vec![],
        };
        let prepared = Self {
            id: call.id.clone(),
            operation: call.op,
            args,
            observations,
            workspace: workspace.to_owned(),
            definition: definition.clone(),
            candidate,
        };
        if serde_json::to_vec(&prepared)?.len() > 256 * 1024 {
            return Err(Error::InvalidArgument(
                "prepared action exceeds the 256 KiB approval limit".into(),
            ));
        }
        Ok(prepared)
    }

    pub(super) fn awaiting(
        self,
        host: &WorkspaceHost,
        queued: QueuedToolCall,
        partial: ToolResultEnvelope,
    ) -> Result<AwaitingAction> {
        let request = PermissionRequest {
            id: host.call_id.clone(),
            tool_call_id: host.call_id.clone(),
            turn: host.turn,
            conversation: crate::conversation::address::ConversationAddress::parse(
                &host.actions.agent,
            )
            .map_err(Error::InvalidArgument)?,
            tool: self.operation.name().into(),
            risk: self.definition.risk.clone(),
            summary: display_summary(self.operation.name(), &self.args, Some(256)),
            candidate: self.candidate.clone(),
            source: "workspace_script".into(),
        };
        let facts: WorkspaceScriptFacts =
            serde_json::from_value(partial.structured.clone().unwrap_or(Value::Null))?;
        let continuation = Continuation {
            action: self,
            facts,
        };
        Ok(AwaitingAction {
            queued,
            request,
            partial,
            continuation,
        })
    }
}

impl AwaitingAction {
    pub(in crate::query) fn persist(&self, events_path: &Path) -> Result<()> {
        EventStore::open(events_path)?.append(EventPayload::Unknown(json!({
            "kind": "workspace.deferred", "ts": now_timestamp()?,
            "agent": self.request.conversation.as_str(), "turn": self.request.turn,
            "tool_call_id": self.request.tool_call_id, "continuation": self.continuation,
        })))?;
        Ok(())
    }

    pub(in crate::query) fn blocked_job(self) -> ToolResultEnvelope {
        let reason = format!(
            "{} requires approval. The Job stopped before dispatching it.",
            self.request.tool
        );
        ToolResultEnvelope {
            status: "blocked".into(),
            summary: "workspace Job stopped at an approval boundary".into(),
            model_content: format!("{reason}\n{}", self.partial.model_content),
            ..self.partial
        }
    }
}

pub(super) fn load(pending: &PendingRun, id: &str) -> Result<Option<Continuation>> {
    let mut cursor = pending.turn.saturating_sub(1);
    loop {
        let page = EventStore::read_page(&pending.events_path, cursor, 64)?;
        if page.is_empty() {
            return Ok(None);
        }
        for event in page {
            cursor = event.id;
            if let EventPayload::Unknown(mut fact) = event.payload {
                if fact["kind"] == "workspace.deferred"
                    && fact["tool_call_id"] == id
                    && fact["turn"] == pending.turn
                    && fact["agent"] == pending.conversation.as_str()
                {
                    return Ok(Some(serde_json::from_value(fact["continuation"].take())?));
                }
            }
        }
    }
}

impl Continuation {
    pub(super) fn decline(self) -> ToolResultEnvelope {
        self.complete(ToolResultEnvelope::blocked(
            "host denied prepared workspace action",
            "the prepared operation was not dispatched",
        ))
    }

    pub(super) fn cancel(self) -> ToolResultEnvelope {
        self.complete(ToolResultEnvelope::cancelled(
            "prepared action cancelled before dispatch",
        ))
    }

    pub(super) async fn resume(
        self,
        host: &WorkspaceHost,
        execution: Arc<ScriptExecution>,
        stop: Arc<Notify>,
    ) -> ToolResultEnvelope {
        for observation in &self.action.observations {
            execution.remember_observation(observation.clone());
        }
        let watcher = {
            let execution = execution.clone();
            tokio::spawn(async move {
                stop.notified().await;
                execution.stop();
            })
        };
        let result = self.run(host, execution).await.unwrap_or_else(|err| {
            ToolResultEnvelope::error("prepared action failed", err.to_string())
        });
        watcher.abort();
        self.complete(result)
    }

    async fn run(
        &self,
        host: &WorkspaceHost,
        execution: Arc<ScriptExecution>,
    ) -> Result<ToolResultEnvelope> {
        let action = &self.action;
        let name = action.operation.name();
        if !host.methods.contains(&action.definition) || host.actions.workspace != action.workspace
        {
            return Ok(ToolResultEnvelope::blocked(
                "prepared workspace action changed",
                "the saved definition or workspace no longer matches",
            ));
        }
        let mut checked = action.args.clone();
        let check = if action.operation == ScriptOperation::RunCommand {
            crate::tool::builtin::run_command::prepare_command_args(
                &mut checked,
                &host.actions.workspace,
                &action.definition,
            )
            .map(|_| ())
        } else {
            prepare_file_target(name, &mut checked, &host.actions.workspace)
        };
        if let Err(result) = check {
            return Ok(result);
        }
        let candidate = permission_candidate(&host.home, &host.actions.workspace, name, &checked);
        let policy = crate::permission::load_project_policy(&host.policy_path)?;
        let guard = crate::permission::decide_tool_call(
            name,
            &action.definition.risk,
            &candidate,
            &policy,
            &[],
        );
        if checked != action.args
            || candidate != action.candidate
            || guard.kind == GateDecisionKind::Deny
        {
            return Ok(ToolResultEnvelope::blocked(
                "prepared target or permission changed",
                "the saved operation was not dispatched",
            ));
        }
        let capacity = host.actions.session.workspace_execution_host().operations();
        let cancel = execution.cancellation();
        let _permit = tokio::select! {
            biased;
            _=cancel.notified()=>return Ok(ToolResultEnvelope::cancelled("stopped before dispatch")),
            permit=capacity.acquire()=>permit.map_err(|_|Error::InvalidArgument("workspace capacity closed".into()))?,
        };
        host.execute_authorized(
            &action.id,
            action.operation,
            action.args.clone(),
            execution,
            !action.definition.read_only,
        )
        .await
    }

    fn complete(mut self, result: ToolResultEnvelope) -> ToolResultEnvelope {
        let status = match result.status.as_str() {
            "ok" => WorkspaceScriptActionStatus::Ok,
            "blocked" => WorkspaceScriptActionStatus::Blocked,
            "cancelled" => WorkspaceScriptActionStatus::Cancelled,
            _ => WorkspaceScriptActionStatus::Error,
        };
        let mut detail = result.structured.unwrap_or(Value::Null);
        if let Some(object) = detail.as_object_mut() {
            for field in [
                "raw_text",
                "raw_text_after",
                "content",
                "segments",
                "inline_pipes",
                "matches",
                "entries",
            ] {
                object.remove(field);
            }
        }
        if let Some(action) = self
            .facts
            .actions
            .iter_mut()
            .find(|action| action.id == self.action.id)
        {
            action.status = status;
            action.result = detail;
            action.summary = result.summary.clone();
            action.truncated = result.truncated;
        }
        let summary = format!(
            "prepared {}: {}",
            self.action.operation.name(),
            result.summary
        );
        let mut content = result.model_content;
        let mut boundary = content.len().min(8192);
        while !content.is_char_boundary(boundary) {
            boundary -= 1;
        }
        content.truncate(boundary);
        ToolResultEnvelope { status:result.status, summary:summary.clone(),truncated:result.truncated,
            model_content:format!("{summary}\n{content}\nThe script stopped for approval. Continue from the recorded actions."),
            structured:Some(json!(self.facts)),faults:result.faults,supplied_full_write:false }
    }
}
