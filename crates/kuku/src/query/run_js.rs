use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, OwnedSemaphorePermit, oneshot};

use crate::event::{EventPayload, EventStore};
use crate::permission::{ActionAuthorization, ToolAuthorization, ToolAuthorizer, authorize_action};
use crate::session::runtime::ScriptOwner;
use crate::tool::builtin::common::{ExecutorObservation, changed_file_target, prepare_file_target};
use crate::tool::run_js::{
    self, ScriptCall, ScriptExecution, ScriptFuture, ScriptHost, ScriptOperation, WorkspaceActions,
};
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, Result, Session};

use super::helpers::{display_summary, now_timestamp, permission_candidate};
use super::job_slots::ForegroundJob;
use super::types::{
    ExecSlot, PendingRun, PermissionRequest, QueuedToolCall, SlotEvent, ToolKind, WorkspaceAccess,
};

pub(super) fn spawn(
    pending: &PendingRun,
    queued: &QueuedToolCall,
    (job, completion): (Box<ForegroundJob>, oneshot::Sender<SlotEvent>),
) -> Result<ExecSlot> {
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
    let limits = pending.query.runtime_limits.workspace_script.clone();
    let host_capacity = session.workspace_execution_host();
    let host = Arc::new(WorkspaceHost {
        owner: ScriptOwner {
            detached: job.detached_flag(),
            execution_id: pending
                .query
                .runtime_execution
                .expect("Session owns the script"),
        },
        host_result_limit: limits.host_bytes / 2,
        actions: WorkspaceActions {
            session,
            agent: pending.conversation.as_str().into(),
            original_turn: pending.turn,
            tool_call_id: queued.tool_call.id.clone(),
            workspace: pending.workspace.clone(),
            preview_bytes: pending.query.runtime_limits.max_preview_bytes,
            io_limits: pending.query.runtime_limits.workspace_io,
            max_command_seconds: pending.query.runtime_limits.max_command_seconds,
            max_command_memory_bytes: pending.query.runtime_limits.max_command_memory_bytes,
        },
        call_id: queued.tool_call.id.clone(),
        turn: pending.turn,
        home: pending.kuku_home.clone(),
        plugins: pending.plugin_registry.clone(),
        source: pending.query.input_source.clone(),
        authority: pending
            .query
            .authorizer
            .clone()
            .expect("Session provides authority"),
        methods: selected_methods(pending),
    });
    let code = queued.tool_call.args["code"].as_str().map(str::to_owned);
    let cancel = Arc::new(Notify::new());
    let stop = cancel.clone();
    tokio::spawn(async move {
        let outcome = match code {
            Some(code) => run_js::execute(code, host, limits, host_capacity, stop).await,
            None => ToolResultEnvelope::error(
                "missing script code",
                "run_js requires a JavaScript function body in code",
            ),
        };
        let _ = completion.send(SlotEvent::Done(outcome));
    });
    Ok(ExecSlot {
        tool_call_id: queued.tool_call.id.clone(),
        conversation: Some(pending.conversation.clone()),
        kind: ToolKind::Simple,
        workspace_access: WorkspaceAccess::Exclusive,
        label: queued.display_summary.clone(),
        cancel,
        job: Some(job),
    })
}

pub(super) fn method_hint(pending: &PendingRun, name: &str) -> Option<String> {
    super::run::find_tool_definition(pending, "run_js")?;
    selected_methods(pending)
        .iter()
        .any(|method| method.name == name)
        .then(|| format!("Use run_js and call tools.{name}(args) inside the JavaScript code."))
}

fn selected_methods(pending: &PendingRun) -> Vec<ToolDefinition> {
    run_js::method_definitions(|op| {
        let definition = super::run::find_tool_definition(pending, op.name())?;
        Some(if op == ScriptOperation::ReadOutput {
            super::runtime_tools::output::definition_local()
        } else {
            definition.clone()
        })
    })
}

struct WorkspaceHost {
    owner: ScriptOwner,
    actions: WorkspaceActions,
    host_result_limit: usize,
    methods: Vec<ToolDefinition>,
    call_id: String,
    turn: u64,
    home: PathBuf,
    plugins: Option<Arc<crate::plugin::PluginRegistry>>,
    source: crate::InputSource,
    authority: Arc<dyn ToolAuthorizer>,
}

impl ScriptHost for WorkspaceHost {
    fn methods(&self) -> Vec<ToolDefinition> {
        self.methods.clone()
    }

    fn retain_return(
        &self,
        json: String,
    ) -> ScriptFuture<'_, Result<Option<crate::OutputReference>>> {
        Box::pin(async move { self.actions.retain_script_return(json).await.map(Some) })
    }

    fn dispatch(
        &self,
        call: ScriptCall,
        execution: Arc<ScriptExecution>,
        permit: OwnedSemaphorePermit,
    ) -> ScriptFuture<'_, ToolResultEnvelope> {
        Box::pin(async move {
            self.perform(call, execution, permit)
                .await
                .unwrap_or_else(|error| {
                    ToolResultEnvelope::error("workspace action failed", error.to_string())
                })
        })
    }
}
impl WorkspaceHost {
    fn has_step_input(&self) -> Result<bool> {
        if self.owner.detached.load(Ordering::Acquire) {
            return Ok(false);
        }
        self.actions.session.has_step_input(&self.actions.agent)
    }

    async fn perform(
        &self,
        call: ScriptCall,
        execution: Arc<ScriptExecution>,
        mut permit: OwnedSemaphorePermit,
    ) -> Result<ToolResultEnvelope> {
        if self.has_step_input()? {
            execution.stop_for_new_input();
            return Ok(ToolResultEnvelope::cancelled(
                "new input arrived at the workspace action boundary",
            ));
        }
        let name = call.op.name();
        let permission_name = call.op.permission_name();
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
            permission_name,
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
        if call.op == ScriptOperation::SearchOutput {
            super::runtime_tools::output::prepare_search(
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
                definition,
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
        let candidate =
            permission_candidate(&self.home, &self.actions.workspace, permission_name, &args);
        let mut schema = super::runtime_tools::schema(definition);
        schema.name = permission_name.into();
        let action = ToolAuthorization {
            session_id: self.actions.session.id().into(),
            agent: self.actions.agent.clone(),
            source: self.source.clone(),
            tool_call_id: id.clone(),
            tool: schema,
            args: args.clone(),
            workspace,
        };
        let decision = authorize_action(
            self.actions.session.permission_mode(),
            definition.read_only,
            self.authority.as_ref(),
            &action,
        )?;
        if self.has_step_input()? {
            execution.stop_for_new_input();
        }
        if execution.is_cancelled() {
            return Ok(ToolResultEnvelope::cancelled(
                "run_js stopped before dispatch",
            ));
        }
        let mutating = !definition.read_only;
        let target = args
            .get("path")
            .or(args.get("id"))
            .or(args.get("name"))
            .unwrap_or(&Value::Null);
        match decision {
            ActionAuthorization::Allow => {}
            ActionAuthorization::Deny { source, reason } => {
                self.record(
                    &id,
                    name,
                    target,
                    "blocked",
                    json!({"source":source, "reason":reason}),
                )?;
                return Ok(ToolResultEnvelope::blocked(
                    reason,
                    format!("{name} was not dispatched because {reason}."),
                ));
            }
            ActionAuthorization::Defer { source } => {
                drop(permit);
                let allowed = self
                    .approve(&action, source, &definition.risk, &candidate, &execution)
                    .await?;
                if execution.is_cancelled() {
                    return Ok(ToolResultEnvelope::cancelled(
                        "run_js stopped before dispatch",
                    ));
                }
                if !allowed {
                    self.record(
                        &id,
                        name,
                        target,
                        "blocked",
                        json!({"source":source, "reason":"approval denied"}),
                    )?;
                    return Ok(ToolResultEnvelope::blocked(
                        "workspace action denied",
                        "the prepared operation was not dispatched",
                    ));
                }
                if self.has_step_input()? {
                    execution.stop_for_new_input();
                }
                let Some(resumed_permit) = execution.operation_permit().await else {
                    return Ok(ToolResultEnvelope::cancelled(
                        "run_js stopped before dispatch",
                    ));
                };
                permit = resumed_permit;
                if let Some(reason) = authorize_action(
                    self.actions.session.permission_mode(),
                    definition.read_only,
                    self.authority.as_ref(),
                    &action,
                )?
                .rejection_after_approval(source)
                {
                    self.record(
                        &id,
                        name,
                        target,
                        "blocked",
                        json!({"source":"active_policy", "reason":reason}),
                    )?;
                    return Ok(ToolResultEnvelope::blocked(
                        reason,
                        "the approved operation no longer satisfies the active policy",
                    ));
                }
            }
        }
        let _permit = permit;
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

    async fn approve(
        &self,
        action: &ToolAuthorization,
        source: &str,
        risk: &str,
        candidate: &str,
        execution: &ScriptExecution,
    ) -> Result<bool> {
        let request = PermissionRequest {
            id: format!(
                "script:{}:{}:{}",
                self.actions.agent, self.turn, action.tool_call_id
            ),
            tool_call_id: self.call_id.clone(),
            turn: self.turn,
            conversation: crate::conversation::address::ConversationAddress::parse(
                &self.actions.agent,
            )
            .map_err(Error::InvalidArgument)?,
            tool: action.tool.name.clone(),
            risk: risk.into(),
            summary: display_summary(&action.tool.name, &action.args, Some(256)),
            candidate: candidate.into(),
            source: source.into(),
            workspace: Some(action.workspace.clone()),
            arguments: Some(action.args.clone()),
        };
        let id = request.id.clone();
        let session = &self.actions.session;
        let approval = session.request_script_permission(request, self.owner.clone())?;
        tokio::select! {
            biased;
            _ = execution.cancelled() => {
                session.end_script_permission(&id, "the script stopped before the operation was approved")?;
                Ok(false)
            }
            reply = approval => Ok(reply.unwrap_or(false)),
        }
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
        if operation == ScriptOperation::RunCommand {
            if let Some(blocked) =
                crate::tool::builtin::run_command::changed_command_directory(&args)
            {
                return Ok(blocked);
            }
        }
        if mutating {
            let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&args)?));
            let payload = self.action_event(
                &id,
                name,
                &target,
                "dispatched",
                json!({"args_sha256":hash}),
            )?;
            if !self.actions.session.mark_script_action_dispatched(
                &self.actions.agent,
                &self.owner,
                payload,
            )? {
                return Ok(ToolResultEnvelope::cancelled(
                    "stopped before prepared action dispatch",
                ));
            }
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
            ScriptOperation::SearchOutput => {
                let session = self.actions.session.clone();
                let agent = self.actions.agent.clone();
                let limit = self.host_result_limit;
                tokio::task::spawn_blocking(move || {
                    super::runtime_tools::output::execute_search(
                        &session,
                        &agent,
                        &args,
                        limit,
                        || execution.is_cancelled(),
                    )
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
                    .execute(operation, local_id, args, execution)
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
        EventStore::open(self.actions.session.events_path())?
            .append(self.action_event(id, name, target, status, facts)?)?;
        Ok(())
    }

    fn action_event(
        &self,
        id: &str,
        name: &str,
        target: &Value,
        status: &str,
        facts: Value,
    ) -> Result<EventPayload> {
        Ok(EventPayload::Unknown(json!({
            "kind":"workspace.action","ts":now_timestamp()?,"agent":self.actions.agent,"turn":self.turn,
            "tool_call_id":self.call_id,"action_id":id,"tool":name,"target":target,"status":status,"facts":facts
        })))
    }
}
