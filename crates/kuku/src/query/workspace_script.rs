mod deferred;

pub(crate) use deferred::AwaitingAction;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Notify};

use crate::event::{EventPayload, EventStore};
use crate::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use crate::tool::builtin::common::ExecutorObservation;
use crate::tool::workspace_script::{
    self, ScriptCall, ScriptExecution, ScriptFuture, ScriptHost, ScriptOperation, WorkspaceActions,
};
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, Result, Session};

use super::helpers::{now_timestamp, permission_candidate};
use super::types::{ExecSlot, PendingRun, QueuedToolCall, SlotEvent, ToolKind};

pub(super) fn spawn(
    pending: &PendingRun,
    queued: &QueuedToolCall,
    event_tx: mpsc::Sender<(String, SlotEvent)>,
) -> Result<ExecSlot> {
    let continuation = deferred::load(pending, &queued.tool_call.id)?;
    let session = Session {
        inner: pending
            .query
            .runtime
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| {
                Error::InvalidArgument("workspace execution requires an open Session".into())
            })?,
    };
    let mut methods: Vec<_> = ScriptOperation::ALL
        .into_iter()
        .filter_map(|op| super::run::find_tool_definition(pending, op.name()).cloned())
        .collect();
    if let Some(reader) = methods
        .iter_mut()
        .find(|method| method.name == "read_output")
    {
        *reader = super::runtime_tools::output::definition_local();
    }
    methods.extend(workspace_script::derived_output_definitions());
    let limits = pending.query.runtime_limits.workspace_script.clone();
    let host_capacity = session.workspace_execution_host();
    let host = Arc::new(WorkspaceHost {
        deferred: Mutex::new(None),
        host_result_limit: limits.host_bytes / 2,
        actions: WorkspaceActions {
            session,
            agent: pending.conversation.as_str().into(),
            workspace: pending.workspace.clone(),
            preview_bytes: pending.query.runtime_limits.max_preview_bytes,
        },
        call_id: queued.tool_call.id.clone(),
        turn: pending.turn,
        policy_path: pending.policy_path.clone(),
        home: pending.kuku_home.clone(),
        plugins: pending.plugin_registry.clone(),
        source: pending.query.input_source.clone(),
        authority: pending
            .query
            .authorizer
            .clone()
            .expect("Session provides authority"),
        methods,
    });
    let code = queued.tool_call.args["code"].as_str().map(str::to_owned);
    let cancel = Arc::new(Notify::new());
    let stop = cancel.clone();
    let id = queued.tool_call.id.clone();
    let original = QueuedToolCall {
        tool_call: queued.tool_call.clone(),
        display_summary: queued.display_summary.clone(),
    };
    tokio::spawn(async move {
        let outcome = match (continuation, code) {
            (Some(continuation), _) => {
                let execution = Arc::new(ScriptExecution::new(limits, host_capacity.operations()));
                continuation.resume(&host, execution, stop).await
            }
            (None, Some(code)) => {
                workspace_script::execute(code, host.clone(), limits, host_capacity, stop).await
            }
            (None, None) => ToolResultEnvelope::error(
                "missing script code",
                "workspace_script requires a JavaScript function body in code",
            ),
        };
        let plan = host.deferred.lock().unwrap().take();
        let event = match plan {
            Some(plan)
                if !outcome.faults.cleanup_incomplete
                    && !outcome.faults.output_persistence_failed =>
            {
                match plan.suspend(&host, original, outcome) {
                    Ok(action) => SlotEvent::WorkspacePermission(Box::new(action)),
                    Err(err) => SlotEvent::Done(ToolResultEnvelope::output_failure(
                        "could not persist workspace approval",
                        err.to_string(),
                        "workspace_approval_failed",
                        false,
                    )),
                }
            }
            _ => SlotEvent::Done(outcome),
        };
        let _ = event_tx.send((id, event)).await;
    });
    Ok(ExecSlot {
        tool_call_id: queued.tool_call.id.clone(),
        conversation: Some(pending.conversation.clone()),
        kind: ToolKind::Simple,
        workspace_ordered: true,
        label: queued.display_summary.clone(),
        cancel,
        nested_permissions: Arc::new(Mutex::new(HashMap::new())),
    })
}

struct WorkspaceHost {
    deferred: Mutex<Option<deferred::PreparedAction>>,
    actions: WorkspaceActions,
    host_result_limit: usize,
    methods: Vec<ToolDefinition>,
    call_id: String,
    turn: u64,
    policy_path: PathBuf,
    home: PathBuf,
    plugins: Option<Arc<crate::plugin::PluginRegistry>>,
    source: crate::InputSource,
    authority: Arc<dyn ToolAuthorizer>,
}

impl ScriptHost for WorkspaceHost {
    fn methods(&self) -> Vec<ToolDefinition> {
        self.methods.clone()
    }

    fn dispatch(
        &self,
        call: ScriptCall,
        execution: Arc<ScriptExecution>,
    ) -> ScriptFuture<'_, ToolResultEnvelope> {
        Box::pin(async move {
            self.perform(call, execution).await.unwrap_or_else(|error| {
                ToolResultEnvelope::error("workspace action failed", error.to_string())
            })
        })
    }
}

impl WorkspaceHost {
    async fn perform(
        &self,
        call: ScriptCall,
        execution: Arc<ScriptExecution>,
    ) -> Result<ToolResultEnvelope> {
        if self.actions.session.has_step_input(&self.actions.agent)? {
            execution.stop();
            return Ok(ToolResultEnvelope::cancelled(
                "new input arrived at the workspace action boundary",
            ));
        }
        let name = call.op.name();
        let Some(definition) = self.methods.iter().find(|method| method.name == name) else {
            return Ok(ToolResultEnvelope::blocked(
                "tool unavailable to this Agent",
                name,
            ));
        };
        let id = format!("{}/{}", self.call_id, call.id);
        let mut context = vec![];
        let prepared = super::tool_exec::prepare_host_action(
            self.plugins.as_deref(),
            self.actions.session.events_path(),
            &self.actions.workspace,
            self.turn,
            name,
            &call.args,
            &id,
            &mut context,
        )
        .await?;
        if let Some(block) = prepared.block {
            return Ok(ToolResultEnvelope::blocked(
                "workspace action blocked by hook",
                block.reason,
            ));
        }
        let mut args = prepared.args;
        if call.op == ScriptOperation::ReadOutput {
            super::runtime_tools::output::prepare_local(
                &self.actions.session,
                &self.actions.agent,
                self.turn,
                &mut args,
            )?;
        }
        if call.op == ScriptOperation::SaveValue {
            super::runtime_tools::values::prepare_save(
                &self.actions.session,
                &self.actions.agent,
                self.turn,
                &args,
            )?;
        }
        let workspace = if call.op == ScriptOperation::RunCommand {
            match crate::tool::builtin::run_command::prepare_command_args(
                &mut args,
                &self.actions.workspace,
            ) {
                Ok(cwd) => cwd,
                Err(result) => return Ok(result),
            }
        } else {
            if let Err(result) = prepare_file_target(name, &mut args, &self.actions.workspace) {
                return Ok(result);
            }
            self.actions.workspace.clone()
        };
        let candidate = permission_candidate(&self.home, &self.actions.workspace, name, &args);
        let action = ToolAuthorization {
            session_id: self.actions.session.id().into(),
            agent: self.actions.agent.clone(),
            source: self.source.clone(),
            tool_call_id: id.clone(),
            tool: super::runtime_tools::schema(definition),
            args: args.clone(),
            workspace,
        };
        let policy = crate::permission::load_project_policy(&self.policy_path)?;
        let decision = super::runtime_tools::authorize(
            &policy,
            definition,
            &candidate,
            self.authority.as_ref(),
            &action,
        )?;
        if self.actions.session.has_step_input(&self.actions.agent)? {
            execution.stop();
        }
        if execution.is_cancelled() {
            return Ok(ToolResultEnvelope::cancelled(
                "workspace script stopped before dispatch",
            ));
        }
        let mutating = !definition.read_only;
        match decision {
            AuthorizationDecision::Allow => {}
            AuthorizationDecision::Deny => {
                let target = args
                    .get("path")
                    .or(args.get("id"))
                    .or(args.get("name"))
                    .unwrap_or(&Value::Null);
                self.record(
                    &id,
                    name,
                    target,
                    "blocked",
                    json!({"reason":"host denied workspace action"}),
                )?;
                return Ok(ToolResultEnvelope::blocked(
                    "host denied workspace action",
                    format!("{name} was not dispatched"),
                ));
            }
            AuthorizationDecision::Defer
                if matches!(
                    call.op,
                    ScriptOperation::CreateOutput
                        | ScriptOperation::AppendOutput
                        | ScriptOperation::FinishOutput
                ) =>
            {
                let reason = "Output drafts expire with their script invocation. Use an immediate authorization decision for draft actions.";
                self.record(
                    &id,
                    name,
                    args.get("id").unwrap_or(&Value::Null),
                    "blocked",
                    json!({"reason":reason}),
                )?;
                return Ok(ToolResultEnvelope::blocked(
                    "output draft action needs immediate authorization",
                    reason,
                ));
            }
            AuthorizationDecision::Defer => {
                let mut waiting = self.deferred.lock().unwrap();
                if waiting.is_some() {
                    return Ok(ToolResultEnvelope::cancelled(
                        "another script operation is awaiting approval",
                    ));
                }
                let action = deferred::PreparedAction::new(
                    &call,
                    args,
                    definition,
                    candidate,
                    &self.actions.workspace,
                    &execution,
                )?;
                *waiting = Some(action);
                execution.stop();
                return Ok(ToolResultEnvelope::blocked(
                    "workspace action awaiting approval",
                    "the script stopped before dispatch",
                ));
            }
        }
        let supplied_full_write = call.op == ScriptOperation::WriteFile
            && call.args["content"]
                .as_str()
                .is_some_and(|content| args["content"].as_str() == Some(content));
        let mut result = self
            .execute_authorized(&call.id, call.op, args, execution.clone(), mutating)
            .await?;
        if supplied_full_write {
            if let Some(observation) = ExecutorObservation::committed_write(&result) {
                execution.stage_observation(&call.id, observation);
            }
        }
        if !context.is_empty() {
            result.model_content.push('\n');
            result.model_content.push_str(&context.join("\n"));
        }
        Ok(result)
    }

    async fn execute_authorized(
        &self,
        local_id: &str,
        operation: ScriptOperation,
        args: Value,
        execution: Arc<ScriptExecution>,
        mutating: bool,
    ) -> Result<ToolResultEnvelope> {
        let id = format!("{}/{local_id}", self.call_id);
        let name = operation.name();
        let output_id =
            crate::session::runtime::command_output_id(&self.actions.agent, self.turn, &id);
        let target = args
            .get("path")
            .or(args.get("id"))
            .or(args.get("name"))
            .cloned()
            .unwrap_or(Value::Null);
        if execution.is_cancelled() {
            return Ok(ToolResultEnvelope::cancelled(
                "stopped before prepared action dispatch",
            ));
        }
        if let Some(blocked) = changed_file_target(name, &args, &self.actions.workspace) {
            return Ok(blocked);
        }
        if mutating {
            let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&args)?));
            self.record(
                &id,
                name,
                &target,
                "dispatched",
                json!({"args_sha256":hash}),
            )?;
        }
        let result = match operation {
            ScriptOperation::ReadOutput => {
                let session = self.actions.session.clone();
                let agent = self.actions.agent.clone();
                let limit = self.host_result_limit;
                tokio::task::spawn_blocking(move || {
                    super::runtime_tools::output::execute_local(&session, &agent, &args, limit)
                })
                .await
                .map_err(|err| Error::InvalidEventStream(err.to_string()))?
            }
            ScriptOperation::SaveValue => {
                Box::pin(super::runtime_tools::values::save(
                    &self.actions.session,
                    &self.actions.agent,
                    &args,
                ))
                .await
            }
            ScriptOperation::ListValues => super::runtime_tools::values::list(
                &self.actions.session,
                &self.actions.agent,
                &args,
            ),
            _ => {
                self.actions
                    .execute(operation, local_id, args, &output_id, execution)
                    .await
            }
        };
        let result = result.unwrap_or_else(|e| {
            ToolResultEnvelope::error("workspace action failed", e.to_string())
        });
        if mutating {
            let mut receipt = json!({"truncated":result.truncated,"cleanup_incomplete":result.faults.cleanup_incomplete});
            if let Some(detail) = &result.structured {
                if operation == ScriptOperation::SaveValue {
                    if let Some(reference) = detail.get("reference") {
                        receipt["output_ref"] = reference.clone();
                    }
                }
                if detail.get("commit_state").is_some() {
                    receipt["mutation"] = detail["kind"].clone();
                }
                for key in [
                    "commit_state",
                    "content_hash_before",
                    "content_hash_after",
                    "bytes_written",
                    "bytes_removed",
                    "exit_code",
                    "termination",
                    "output_ref",
                    "capture_complete",
                    "quota_truncated",
                ] {
                    if let Some(fact) = detail.get(key) {
                        receipt[key] = fact.clone();
                    }
                }
            }
            if let Err(err) = self.record(&id, name, &target, &result.status, receipt) {
                return Ok(ToolResultEnvelope::output_failure(
                    "action receipt could not be saved",
                    err.to_string(),
                    "workspace_receipt_failed",
                    result.faults.cleanup_incomplete,
                ));
            }
        }
        Ok(result)
    }

    fn record(
        &self,
        id: &str,
        name: &str,
        target: &Value,
        status: &str,
        facts: Value,
    ) -> Result<()> {
        EventStore::open(self.actions.session.events_path())?.append(EventPayload::Unknown(json!({
            "kind":"workspace.action","ts":now_timestamp()?,"agent":self.actions.agent,"turn":self.turn,
            "tool_call_id":self.call_id,"action_id":id,"tool":name,"target":target,"status":status,"facts":facts
        })))?;
        Ok(())
    }
}

pub(super) fn prepare_file_target(
    name: &str,
    args: &mut Value,
    workspace: &Path,
) -> std::result::Result<(), ToolResultEnvelope> {
    if let Some(path) = crate::tool::builtin::prepare_workspace_path(name, args, workspace)? {
        let relative = path.strip_prefix(workspace).unwrap_or(&path);
        let target = relative.to_str().ok_or_else(|| {
            ToolResultEnvelope::error("invalid file path", "target must be valid Unicode")
        })?;
        args["path"] = if target.is_empty() { "." } else { target }.into();
    }
    Ok(())
}

pub(super) fn changed_file_target(
    name: &str,
    args: &Value,
    workspace: &Path,
) -> Option<ToolResultEnvelope> {
    let mut current = args.clone();
    if let Err(result) = prepare_file_target(name, &mut current, workspace) {
        return Some(result);
    }
    (current != *args).then(|| {
        ToolResultEnvelope::blocked(
            "workspace target changed after preparation",
            "read the new target before requesting another action",
        )
    })
}

pub(super) fn declined(pending: &PendingRun, id: &str) -> Result<Option<ToolResultEnvelope>> {
    Ok(deferred::load(pending, id)?.map(deferred::Continuation::decline))
}

pub(crate) fn cancelled_deferred_script(continuation: Value) -> Result<ToolResultEnvelope> {
    Ok(serde_json::from_value::<deferred::Continuation>(continuation)?.cancel())
}

pub(super) fn cancelled_permission(
    waiting: &super::types::PendingPermission,
) -> Result<ToolResultEnvelope> {
    if waiting.request.source == "workspace_script" {
        if let Some(action) = deferred::load(&waiting.pending, &waiting.request.tool_call_id)? {
            return Ok(action.cancel());
        }
    }
    Ok(ToolResultEnvelope::cancelled(
        "permission request cancelled",
    ))
}
