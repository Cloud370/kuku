mod agent_status;
pub(super) mod jobs;
mod media;
pub(crate) mod output;
pub(crate) mod values;

use crate::context::ToolSchema;
use crate::event::{EventPayload, EventStore};
use crate::permission::{ActionAuthorization, authorize_action};
use crate::session::runtime::JobControl;
use crate::tool::builtin::common::{changed_file_target, prepare_file_target};
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, Result};

use super::helpers::{
    append_permission_decision, append_permission_request, now_timestamp, permission_candidate,
    permission_rule,
};
use super::job_slots::ForegroundJob;
use super::run::find_tool_definition;
use super::tool_exec::{execute_tool_call, run_tool_pre_hooks};
use super::types::{
    ExecSlot, PendingPermission, PendingRun, PendingStep, PermissionChoice, PermissionRequest,
    QueuedToolCall, SlotEvent, ToolKind, ToolPreparation, UiEvent, WorkspaceAccess,
};

pub(super) async fn advance(
    mut pending: PendingRun,
    mut queued: QueuedToolCall,
    event_tx: tokio::sync::mpsc::Sender<(String, SlotEvent)>,
    active: usize,
) -> Result<PendingStep> {
    if let Some(request) = pending.take_resumed_permission_request(&queued.tool_call.id) {
        if active > 0 {
            pending.resumed_permission_requests.push_front(request);
            pending.queued_tool_calls.push_front(queued);
            return Ok(PendingStep::Pending {
                pending: Box::new(pending),
                slot: None,
                event: None,
            });
        }
        pending.queued_tool_calls.push_front(queued);
        return Ok(PendingStep::NeedPermission(Box::new(PendingPermission {
            pending,
            request,
        })));
    }
    super::provider::ensure_resolved(&mut pending)?;
    let Some(definition) = find_tool_definition(&pending, &queued.tool_call.name).cloned() else {
        let name = &queued.tool_call.name;
        let mut message = format!("unknown tool: {name}");
        if let Some(hint) = super::run_js::method_hint(&pending, name) {
            message.push_str(". ");
            message.push_str(&hint);
        }
        let result = ToolResultEnvelope::error("unknown tool", message);
        return finish_inline(pending, queued, result);
    };
    match prepare(&mut pending, &mut queued, &definition).await {
        Ok(Some(reason)) => {
            return finish_inline(
                pending,
                queued,
                ToolResultEnvelope::blocked(&reason, &reason),
            );
        }
        Err(error) => {
            return finish_inline(
                pending,
                queued,
                ToolResultEnvelope::error("tool preparation failed", error.to_string()),
            );
        }
        Ok(None) => {}
    }
    let action = prepared_authorization(&pending, &queued, &definition);
    let candidate = permission_candidate(
        &pending.kuku_home,
        &pending.workspace,
        &action.tool.name,
        &queued.tool_call.args,
    );
    let decision = match authorize_prepared(&pending, &definition, &action) {
        Ok(decision) => decision,
        Err(error) => {
            return finish_inline(
                pending,
                queued,
                ToolResultEnvelope::error(
                    "host authorization failed; action was not dispatched",
                    error.to_string(),
                ),
            );
        }
    };
    match decision {
        ActionAuthorization::Defer { source } => {
            if active > 0 {
                pending.queued_tool_calls.push_front(queued);
                return Ok(PendingStep::Pending {
                    pending: Box::new(pending),
                    slot: None,
                    event: None,
                });
            }
            let request = PermissionRequest {
                id: queued.tool_call.id.clone(),
                conversation: pending.conversation.clone(),
                turn: pending.turn,
                tool_call_id: queued.tool_call.id.clone(),
                tool: queued.tool_call.name.clone(),
                risk: definition.risk,
                summary: queued.display_summary.clone(),
                candidate,
                source: source.into(),
                workspace: Some(action.workspace),
                arguments: Some(action.args),
            };
            append_permission_request(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                &request,
            )?;
            pending.queued_tool_calls.push_front(queued);
            Ok(PendingStep::NeedPermission(Box::new(PendingPermission {
                pending,
                request,
            })))
        }
        ActionAuthorization::Deny { source, reason } => {
            append_permission_decision(
                &pending.events_path,
                pending.turn,
                &queued.tool_call.id,
                PermissionChoice::Deny,
                source,
                &permission_rule(
                    &pending.kuku_home,
                    &pending.workspace,
                    &queued.tool_call.name,
                    &queued.tool_call.args,
                ),
            )?;
            pending.record_tool_denied();
            finish_inline(
                pending,
                queued,
                ToolResultEnvelope::blocked(
                    reason,
                    format!("The action was not dispatched because {reason}."),
                ),
            )
        }
        ActionAuthorization::Allow => {
            append_permission_decision(
                &pending.events_path,
                pending.turn,
                &queued.tool_call.id,
                PermissionChoice::Once,
                "host",
                &permission_rule(
                    &pending.kuku_home,
                    &pending.workspace,
                    &queued.tool_call.name,
                    &queued.tool_call.args,
                ),
            )?;
            dispatch(pending, queued, event_tx, active).await
        }
    }
}

pub(super) async fn dispatch(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    event_tx: tokio::sync::mpsc::Sender<(String, SlotEvent)>,
    active: usize,
) -> Result<PendingStep> {
    if let Some(result) = changed_file_target(
        &queued.tool_call.name,
        &queued.tool_call.args,
        &pending.workspace,
    ) {
        return finish_inline(pending, queued, result);
    }
    if active >= pending.query.runtime_limits.max_tools_per_agent {
        return finish_inline(
            pending,
            queued,
            ToolResultEnvelope::blocked("tool capacity reached", "the action was not dispatched"),
        );
    }
    if pending.query.runtime.is_none() {
        return dispatch_standalone(pending, queued, event_tx).await;
    }
    if super::helpers::is_inline_skill_tool(&queued.tool_call.name)
        || is_inline(&queued.tool_call.name)
    {
        mark_dispatched(&pending, &queued)?;
        pending.record_tool_call(&queued.tool_call.name);
        let result = Box::pin(execute_tool_call(&mut pending, &queued.tool_call)).await?;
        return finish_inline_event(pending, queued, result);
    }
    if let Some(registered) = pending
        .query
        .tool_adapters
        .get(&queued.tool_call.name)
        .cloned()
    {
        let output = match output_spec(&pending, &queued.tool_call.id) {
            Ok(output) => output,
            Err(error) => return output_admission_failed(pending, queued, error),
        };
        mark_dispatched(&pending, &queued)?;
        let definition = find_tool_definition(&pending, &queued.tool_call.name)
            .expect("authorized tool definition exists");
        let action = crate::permission::ToolAuthorization {
            session_id: pending.session_id.clone(),
            agent: pending.conversation.as_str().into(),
            source: pending.query.input_source.clone(),
            tool_call_id: queued.tool_call.id.clone(),
            tool: schema(definition),
            args: queued.tool_call.args.clone(),
            workspace: pending.workspace.clone(),
        };
        let session = runtime_session(&pending)?;
        let slot = super::adapters::spawn_tool(
            registered,
            action,
            output,
            pending.query.runtime_limits.clone(),
            session.workspace_execution_host(),
            event_tx,
        );
        return start_slot(pending, queued, slot);
    }
    if let Some(control) = JobControl::for_tool(&queued.tool_call.name) {
        let session = runtime_session(&pending)?;
        mark_dispatched(&pending, &queued)?;
        let slot = super::slots::spawn_job_control(session, &pending, &queued, control, event_tx);
        return start_slot(pending, queued, slot);
    }
    if matches!(queued.tool_call.name.as_str(), "run_command" | "run_js") {
        return start_job_capable_slot(pending, queued, event_tx, active);
    }
    if queued.tool_call.name == "view_image" {
        let session = runtime_session(&pending)?;
        mark_dispatched(&pending, &queued)?;
        let slot = media::spawn(session, &pending, &queued, event_tx);
        return start_slot(pending, queued, slot);
    }
    let prior_events = if crate::tool::dispatch::uses_prior_events(&queued.tool_call.name) {
        runtime_session(&pending)?.read_agent_events(pending.conversation.as_str())?
    } else {
        Vec::new()
    };
    let output = if queued.tool_call.name == "fetch_url" {
        match output_spec(&pending, &queued.tool_call.id) {
            Ok(output) => Some(output),
            Err(error) => return output_admission_failed(pending, queued, error),
        }
    } else {
        None
    };
    mark_dispatched(&pending, &queued)?;
    let (slot, _) = super::slots::dispatch_tool_slot(super::slots::SlotDispatchArgs {
        capacity: runtime_session(&pending)?.workspace_execution_host(),
        workspace_access: workspace_access(&pending, &queued.tool_call.name),
        limits: pending.query.runtime_limits.clone(),
        output,
        tool_name: queued.tool_call.name.clone(),
        tool_id: queued.tool_call.id.clone(),
        conversation: Some(pending.conversation.clone()),
        args: queued.tool_call.args.clone(),
        summary: queued.display_summary.clone(),
        workspace: pending.workspace.clone(),
        kuku_home: pending.kuku_home.clone(),
        prior_events,
        event_tx,
        config: pending.config.clone(),
        catalog: pending.catalog.clone(),
        events_path: pending.events_path.clone(),
    });
    start_slot(pending, queued, slot)
}

fn start_job_capable_slot(
    pending: PendingRun,
    queued: QueuedToolCall,
    event_tx: tokio::sync::mpsc::Sender<(String, SlotEvent)>,
    active: usize,
) -> Result<PendingStep> {
    let session = runtime_session(&pending)?;
    let agent = pending.conversation.as_str();
    if active + session.active_jobs(agent) >= pending.query.runtime_limits.max_tools_per_agent {
        return finish_inline(
            pending,
            queued,
            ToolResultEnvelope::blocked("tool capacity reached", "the action was not dispatched"),
        );
    }
    let slot = if queued.tool_call.name == "run_command" {
        let output = match output_spec(&pending, &queued.tool_call.id) {
            Ok(output) => output,
            Err(error) => return output_admission_failed(pending, queued, error),
        };
        mark_dispatched(&pending, &queued)?;
        let job = ForegroundJob::start(&pending, &queued, Some(output.id.clone()));
        super::slots::spawn_command_slot(
            queued.tool_call.id.clone(),
            Some(pending.conversation.clone()),
            queued.tool_call.args.clone(),
            queued.display_summary.clone(),
            pending.workspace.clone(),
            event_tx,
            Some(output),
            session.workspace_execution_host(),
            Some(job),
        )
    } else {
        mark_dispatched(&pending, &queued)?;
        super::run_js::spawn(
            &pending,
            &queued,
            ForegroundJob::start(&pending, &queued, None),
        )?
    };
    start_slot(pending, queued, slot)
}

fn start_slot(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    slot: ExecSlot,
) -> Result<PendingStep> {
    pending.record_tool_call(&queued.tool_call.name);
    let event = UiEvent::ToolStart {
        id: queued.tool_call.id,
        tool: queued.tool_call.name,
        summary: queued.display_summary,
        kind: slot.kind.clone(),
    };
    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: Some(slot),
        event: Some(event),
    })
}

async fn dispatch_standalone(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    event_tx: tokio::sync::mpsc::Sender<(String, SlotEvent)>,
) -> Result<PendingStep> {
    if super::helpers::is_inline_skill_tool(&queued.tool_call.name) {
        pending.record_tool_call(&queued.tool_call.name);
        let result = Box::pin(execute_tool_call(&mut pending, &queued.tool_call)).await?;
        return finish_inline_event(pending, queued, result);
    }
    let (slot, _) = super::slots::dispatch_tool_slot(super::slots::SlotDispatchArgs {
        capacity: crate::WorkspaceExecutionHost::default(),
        workspace_access: workspace_access(&pending, &queued.tool_call.name),
        limits: pending.query.runtime_limits.clone(),
        output: None,
        tool_name: queued.tool_call.name.clone(),
        tool_id: queued.tool_call.id.clone(),
        conversation: (!pending.conversation.is_main()).then(|| pending.conversation.clone()),
        args: queued.tool_call.args.clone(),
        summary: queued.display_summary.clone(),
        workspace: pending.workspace.clone(),
        kuku_home: pending.kuku_home.clone(),
        prior_events: EventStore::replay(&pending.events_path)?,
        event_tx,
        config: pending.config.clone(),
        catalog: pending.catalog.clone(),
        events_path: pending.events_path.clone(),
    });
    start_slot(pending, queued, slot)
}

pub(super) fn should_yield(pending: &PendingRun) -> Result<bool> {
    runtime_session(pending)?
        .question_requested_in_turn(pending.conversation.as_str(), pending.turn)
}

pub(super) fn yield_step(pending: PendingRun) -> Result<PendingStep> {
    super::helpers::append_turn_completed(
        &pending.events_path,
        &pending.conversation,
        pending.turn,
    )?;
    let usage = Some(crate::ProviderUsage {
        input_tokens: pending.cumulative.input_tokens,
        output_tokens: pending.cumulative.output_tokens,
        cache_read_input_tokens: pending.cumulative.cache_read_input_tokens,
        cache_creation_input_tokens: pending.cumulative.cache_creation_input_tokens,
    });
    let output = super::types::RunOutput {
        session_id: pending.session_id.clone(),
        conversation: pending.conversation.clone(),
        text: String::new(),
        usage: usage.clone(),
        turn: pending.turn,
        model_request_count: pending.model_request_count,
        thinking_duration_ms: pending.thinking_duration_ms,
        tool_summary: super::types::ToolSummary {
            total_calls: pending.tool_calls,
            names: pending.tool_names.clone(),
            denied: pending.tool_denied,
            errors: pending.tool_errors,
            rounds: pending.tool_rounds,
        },
        plugin_registry: pending.plugin_registry.clone(),
        session_dir: pending
            .events_path
            .parent()
            .expect("events has session parent")
            .to_path_buf(),
        workspace: pending.workspace.clone(),
    };
    Ok(PendingStep::Done(output, usage, pending.turn))
}

pub(super) fn workspace_access(pending: &PendingRun, name: &str) -> WorkspaceAccess {
    if pending.query.runtime.is_none() {
        return super::slots::query_workspace_access(name);
    }
    pending.query.tool_adapters.get(name).map_or_else(
        || super::slots::workspace_access(name),
        |tool| super::adapters::workspace_access(tool.spec.access),
    )
}

pub(super) fn is_inline(name: &str) -> bool {
    matches!(
        name,
        "create_agent"
            | "send_message"
            | "list_agents"
            | "ask_user"
            | "read_output"
            | "search_output"
            | "save_value"
            | "list_values"
            | "list_jobs"
    )
}

pub(super) fn reject(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    reason: &str,
) -> Result<PendingStep> {
    let result =
        ToolResultEnvelope::blocked(reason, format!("The action was not dispatched. {reason}."));
    record_inline_result(&mut pending, &queued, &result)?;
    let event = inline_tool_end(queued.tool_call.id, result);
    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: Some(event),
    })
}

fn prepared_authorization(
    pending: &PendingRun,
    queued: &QueuedToolCall,
    definition: &ToolDefinition,
) -> crate::permission::ToolAuthorization {
    let tool = schema(definition);
    let workspace = if queued.tool_call.name == "run_command" {
        queued.tool_call.args["cwd"]
            .as_str()
            .map_or_else(|| pending.workspace.clone(), std::path::PathBuf::from)
    } else {
        pending.workspace.clone()
    };
    crate::permission::ToolAuthorization {
        session_id: pending.session_id.clone(),
        agent: pending.conversation.as_str().into(),
        source: pending.query.input_source.clone(),
        tool_call_id: queued.tool_call.id.clone(),
        tool,
        args: queued.tool_call.args.clone(),
        workspace,
    }
}

fn authorize_prepared(
    pending: &PendingRun,
    definition: &ToolDefinition,
    action: &crate::permission::ToolAuthorization,
) -> Result<ActionAuthorization> {
    let mode = effective_permission_mode(pending)?;
    let authority = pending
        .query
        .authorizer
        .as_deref()
        .unwrap_or(&crate::permission::TrustedLocal);
    let delegates_authorization = pending.query.runtime.is_some() && definition.name == "run_js";
    authorize_action(
        mode,
        definition.read_only || delegates_authorization,
        authority,
        action,
    )
}

pub(super) fn effective_permission_mode(
    pending: &PendingRun,
) -> Result<crate::permission::PermissionMode> {
    if pending.query.runtime.is_some() {
        Ok(runtime_session(pending)?.permission_mode())
    } else {
        Ok(pending.query.permission_mode)
    }
}

pub(super) fn check_approved(
    pending: &PendingRun,
    queued: &QueuedToolCall,
    source: &str,
) -> Result<Option<&'static str>> {
    let definition = find_tool_definition(pending, &queued.tool_call.name)
        .expect("prepared tool definition was validated");
    if definition.name == "run_command"
        && crate::tool::builtin::run_command::changed_command_directory(&queued.tool_call.args)
            .is_some()
    {
        return Ok(Some("command directory changed after preparation"));
    }
    let action = prepared_authorization(pending, queued, definition);
    Ok(authorize_prepared(pending, definition, &action)?.rejection_after_approval(source))
}

fn output_spec(pending: &PendingRun, id: &str) -> Result<super::slots::RetainedOutputSpec> {
    use crate::tool::builtin::run_command::ModelOutputAccess;

    let agent = pending.conversation.as_str();
    let retention = runtime_session(pending)?.reserve_output(agent, pending.turn, id)?;
    Ok(super::slots::RetainedOutputSpec {
        dir: pending
            .events_path
            .parent()
            .expect("session events has a parent")
            .join("outputs"),
        id: retention.id().into(),
        limits: crate::tool::builtin::run_command::output::OutputLimits {
            max_output_bytes: retention.max_bytes(),
            max_preview_bytes: pending.query.runtime_limits.max_preview_bytes,
        },
        access: match (
            find_tool_definition(pending, "read_output"),
            find_tool_definition(pending, "run_js"),
        ) {
            (Some(_), Some(_)) => ModelOutputAccess::Script,
            (Some(_), None) => ModelOutputAccess::Read,
            (None, _) => ModelOutputAccess::Unavailable,
        },
        max_command_seconds: pending.query.runtime_limits.max_command_seconds,
        max_command_memory_bytes: pending.query.runtime_limits.max_command_memory_bytes,
        retention,
    })
}

fn output_admission_failed(
    pending: PendingRun,
    queued: QueuedToolCall,
    error: Error,
) -> Result<PendingStep> {
    finish_inline(
        pending,
        queued,
        ToolResultEnvelope::error(
            "output reservation failed before dispatch",
            error.to_string(),
        ),
    )
}

pub(crate) fn supported(
    config: &crate::provider::types::ResolvedProvider,
    builtin_adapter: bool,
    name: &str,
) -> bool {
    name != "view_image"
        || config.capabilities.image_input_support(builtin_adapter)
            == crate::config::ImageInputSupport::Supported
}

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    let read_output = output::definition();
    vec![
        crate::tool::run_js::definition(),
        crate::tool::builtin::view_image_definition(),
        crate::tool::builtin::run_command::output::search::definition(&read_output),
        read_output,
        values::save_definition(),
        values::list_definition(),
        jobs::list_definition(),
        jobs::wait_definition(),
        jobs::cancel_definition(),
        definition(
            "ask_user",
            "Persist a question and yield after the current tool batch. Wait for the host to reply to the returned request identity before continuing.",
            serde_json::json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}),
        ),
        definition(
            "create_agent",
            "Create an idle Agent with a standing role and tool settings. Read its successful creation receipt before calling send_message in a later step to assign work. Resolve creation errors before dispatch.",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "address": {"type": "string", "description": "Stable Session-local name."},
                    "instructions": {"type": "string", "description": "Standing role or constraints for this Agent."},
                    "tools": {"type": "array", "items": {"type":"string"}, "description":"Tool names in addition to send_message and list_agents. Omit to use defaults. An empty array gives only the two messaging tools."},
                    "preset": {"type": "string", "description": "Configured Agent preset name. Omit to use the Session defaults. An unknown name returns available examples."},
                    "tier": {"type": "string", "description": "Configured model tier from <kuku_models>. Omit to use the preset or Session default."},
                    "reasoning_effort": {"type": "string", "description": "Supported thinking value from the selected route in <kuku_models>."},
                    "service_tier": {"type": "string", "description": "Supported service tier from <kuku_models>. Omit to inherit. Set only at the user's explicit request."},
                    "workspace": {"type": "string", "description": "Working directory when different from the Session workspace."}
                },
                "required": ["address"]
            }),
        ),
        definition(
            "send_message",
            "Deliver a task or follow-up and any selected value references to an existing Agent. For a new Agent, first confirm its creation receipt. Delivery wakes an idle recipient by default. The receipt confirms delivery. Pass its event_id as receipt with wait_for in list_agents to retrieve the result for this delivery.",
            serde_json::json!({"type":"object","properties":{"to":{"type":"string"},"message":{"type":"string"},"wake":{"type":"boolean","description":"Defaults true and wakes an idle recipient. False only enqueues the message."},"values":values::handoff_schema()},"required":["to","message"]}),
        ),
        definition(
            "list_agents",
            "List compact Agent states. To retrieve a specific delegated result, set wait_for to the recipient address and receipt to the event_id returned by send_message. Waits up to 30 seconds while the recipient is running. Without receipt, wait_for returns the latest execution by address.",
            serde_json::json!({"type":"object","properties":{"wait_for":{"type":"string","minLength":1},"receipt":{"type":"integer","minimum":1,"description":"event_id from your send_message delivery receipt."}}}),
        ),
    ]
}

pub(super) async fn execute(
    pending: &mut PendingRun,
    call: &crate::provider::types::ProviderToolCall,
) -> Result<ToolResultEnvelope> {
    let session = runtime_session(pending)?;
    let agent = pending.conversation.as_str();
    let key = format!(
        "tool:{}:{}:{}",
        pending.conversation.as_str(),
        pending.turn,
        call.id
    );
    let value = match call.name.as_str() {
        "read_output" | "search_output" => {
            return output::execute_tool(session, agent, call).await;
        }
        "save_value" => return values::save(&session, agent, &call.args).await,
        "list_values" => return values::list(&session, agent, &call.args),
        "list_jobs" => return jobs::list(&session, agent, &call.args),
        "create_agent" => {
            let (address, config) = create_agent_input(&call.args)?;
            let preset = config.preset.clone();
            let receipt = session.create_agent_from_tool(&key, &address, config)?;
            let mut receipt = serde_json::to_value(receipt)?;
            let config = session.agent(&address)?.config;
            receipt["initial_status"] = serde_json::json!(crate::AgentStatus::Idle);
            receipt["workspace"] =
                serde_json::json!(session.workspace_for(&config).to_string_lossy());
            if let Ok(selection) = session.selection_for_config(&address, config) {
                receipt["effective_tools"] = serde_json::json!(
                    selection
                        .tools
                        .iter()
                        .map(|tool| &tool.name)
                        .collect::<Vec<_>>()
                );
                receipt["model_selection"] = serde_json::json!({
                    "preset": preset, "model": selection.model,
                    "reasoning_effort": selection.settings.reasoning_effort.or_else(|| selection.capabilities.reasoning_default.map(|level| level.value)),
                    "service_tier": selection.settings.service_tier,
                    "input_modalities": selection.capabilities.input_modalities.map(|entry| entry.value),
                    "output_modalities": selection.capabilities.output_modalities.map(|entry| entry.value),
                    "effective_input_modalities": selection.image_input.modalities(),
                    "output": selection.output,
                    "image_input": selection.image_input,
                });
            }
            receipt
        }
        "send_message" => {
            let input = crate::Input {
                command_id: key,
                source: crate::InputSource::Agent(pending.conversation.as_str().into()),
                target: required(&call.args, "to")?.into(),
                text: required(&call.args, "message")?.into(),
                activation: if call.args["wake"] == false {
                    crate::Activation::Enqueue
                } else {
                    crate::Activation::Wake
                },
                correlation: Some(call.id.clone()),
                timing: crate::InputTiming::NextStep,
                bootstrap_skill: None,
            };
            serde_json::to_value(
                session.send_with_values(input, &values::handoff_values(&call.args)?)?,
            )?
        }
        "list_agents" => {
            if call.args.get("receipt").is_some() && call.args.get("wait_for").is_none() {
                return Err(Error::InvalidArgument("receipt requires wait_for".into()));
            }
            if let Some(target) = call.args.get("wait_for") {
                let address = target
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        Error::InvalidArgument("wait_for requires an Agent address".into())
                    })?;
                if address == pending.conversation.as_str() {
                    return Err(Error::InvalidArgument(
                        "an Agent cannot wait for itself".into(),
                    ));
                }
                if let Some(receipt) = call.args.get("receipt") {
                    let event_id = receipt.as_u64().filter(|id| *id > 0).ok_or_else(|| {
                        Error::InvalidArgument("receipt requires a positive event_id".into())
                    })?;
                    let result = session
                        .wait_for_delivery(
                            pending.conversation.as_str(),
                            address,
                            event_id,
                            std::time::Duration::from_secs(30),
                        )
                        .await?;
                    serde_json::json!([result])
                } else {
                    let waited = session.wait_for_agent(address).await;
                    let snapshot = current_agent_after_wait(waited, || session.agent(address))?;
                    serde_json::json!([agent_status::listing(
                        snapshot,
                        Some(session.events_path())
                    )])
                }
            } else {
                serde_json::Value::Array(
                    session
                        .agents()
                        .into_iter()
                        .map(|agent| agent_status::listing(agent, None))
                        .collect(),
                )
            }
        }
        "ask_user" => serde_json::to_value(
            session.request_question(
                pending.conversation.as_str(),
                pending
                    .query
                    .runtime_execution
                    .expect("session runtime owns this execution"),
                pending.turn,
                &call.id,
                required(&call.args, "question")?,
            )?,
        )?,
        _ => {
            return Err(Error::InvalidArgument(format!(
                "unknown session operation {}",
                call.name
            )));
        }
    };
    Ok(ToolResultEnvelope::ok(
        "session operation committed",
        serde_json::to_string(&value)?,
        value,
    ))
}

pub(super) fn runtime_session(pending: &PendingRun) -> Result<crate::Session> {
    let execution = pending
        .query
        .runtime_execution
        .expect("session runtime owns this execution");
    super::provider::runtime_session(pending, execution)
}

fn current_agent_after_wait<T>(
    waited: Result<T>,
    current: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match waited {
        Err(Error::AgentWaitTimeout) => current(),
        result => result,
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentInput {
    address: String,
    instructions: Option<String>,
    tools: Option<Vec<String>>,
    preset: Option<String>,
    tier: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    workspace: Option<std::path::PathBuf>,
}

fn create_agent_input(args: &serde_json::Value) -> Result<(String, crate::AgentConfig)> {
    use serde::Deserialize;
    let input = CreateAgentInput::deserialize(args)?;
    if input.address.trim().is_empty() {
        return Err(Error::InvalidArgument(
            "tool argument 'address' must not be blank".into(),
        ));
    }
    Ok((
        input.address,
        crate::AgentConfig {
            instructions: input.instructions,
            tools: input.tools,
            preset: input.preset,
            tier: input.tier,
            reasoning_effort: input.reasoning_effort,
            service_tier: input.service_tier,
            workspace: input.workspace,
            ..Default::default()
        },
    ))
}

fn normalize_create_agent_args(args: &mut serde_json::Value) {
    let Some(values) = args.as_object_mut() else {
        return;
    };
    for field in [
        "tier",
        "preset",
        "workspace",
        "reasoning_effort",
        "service_tier",
    ] {
        if values
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.trim().is_empty())
        {
            values.remove(field);
        }
    }
}

pub(super) fn validate_prepared(pending: &PendingRun, queued: &QueuedToolCall) -> Result<()> {
    let call = &queued.tool_call;
    let definition = find_tool_definition(pending, &call.name).ok_or_else(|| {
        Error::InvalidArgument(format!("tool '{}' is no longer available", call.name))
    })?;
    let prepared = queued.preparation.as_ref().ok_or_else(|| {
        Error::InvalidEventStream("pending action has no durable prepared arguments".into())
    })?;
    if prepared.args != call.args || prepared.definition != schema_value(definition) {
        return Err(Error::InvalidArgument(
            "pending action no longer matches its authorized tool definition and arguments".into(),
        ));
    }
    Ok(())
}

async fn prepare(
    pending: &mut PendingRun,
    queued: &mut QueuedToolCall,
    definition: &ToolDefinition,
) -> Result<Option<String>> {
    if let Some(prepared) = &queued.preparation {
        if prepared.definition != schema_value(definition) {
            return Err(Error::InvalidArgument(
                "prepared tool definition changed before dispatch".into(),
            ));
        }
        queued.tool_call.args.clone_from(&prepared.args);
        return Ok(None);
    }
    let result = run_tool_pre_hooks(
        pending,
        &queued.tool_call.name,
        &queued.tool_call.args,
        &queued.tool_call.id,
    )
    .await?;
    if let Some(block) = result.block {
        return Ok(Some(block.reason));
    }
    queued.tool_call.args = result.args;
    prepare_file_target(
        &queued.tool_call.name,
        &mut queued.tool_call.args,
        &pending.workspace,
    )
    .map_err(|result| Error::InvalidArgument(result.model_content))?;
    if queued.tool_call.name == "create_agent" {
        normalize_create_agent_args(&mut queued.tool_call.args);
    }
    if queued.tool_call.name == "run_command" {
        crate::tool::builtin::run_command::prepare_command_args(
            &mut queued.tool_call.args,
            &pending.workspace,
            definition,
        )
        .map_err(|result| Error::InvalidArgument(result.model_content))?;
    }
    if queued.tool_call.name == "run_js"
        && queued
            .tool_call
            .args
            .get("background")
            .is_some_and(|value| !value.is_boolean())
    {
        return Err(Error::InvalidArgument(
            "background must be a boolean".into(),
        ));
    }
    let agent = pending.conversation.as_str();
    let turn = pending.turn;
    if pending.query.runtime.is_some() {
        match queued.tool_call.name.as_str() {
            "read_output" => output::prepare(
                &runtime_session(pending)?,
                agent,
                turn,
                &mut queued.tool_call.args,
            )?,
            "search_output" => output::prepare_search(
                &runtime_session(pending)?,
                agent,
                turn,
                &mut queued.tool_call.args,
            )?,
            "save_value" => values::prepare_save(
                &runtime_session(pending)?,
                agent,
                turn,
                &queued.tool_call.args,
            )?,
            "send_message" => values::prepare_handoff(
                &runtime_session(pending)?,
                agent,
                turn,
                &mut queued.tool_call.args,
            )?,
            _ => {}
        }
    }
    let preparation = ToolPreparation {
        args: queued.tool_call.args.clone(),
        definition: schema_value(definition),
    };
    EventStore::open(&pending.events_path)?.append(EventPayload::Unknown(serde_json::json!({
        "kind":"runtime.tool_prepared", "ts":now_timestamp()?, "agent":pending.conversation.as_str(),
        "turn":pending.turn, "tool_call_id":queued.tool_call.id,
        "args":preparation.args, "definition":preparation.definition,
    })))?;
    queued.preparation = Some(preparation);
    Ok(None)
}

fn mark_dispatched(pending: &PendingRun, queued: &QueuedToolCall) -> Result<()> {
    let execution = pending
        .query
        .runtime_execution
        .expect("session runtime owns this execution");
    super::provider::runtime_session(pending, execution)?.mark_tool_dispatch(
        pending.conversation.as_str(),
        execution,
        pending.turn,
        &queued.tool_call.id,
        &queued.tool_call.name,
    )
}

fn finish_inline(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    result: ToolResultEnvelope,
) -> Result<PendingStep> {
    record_inline_result(&mut pending, &queued, &result)?;
    finish_inline_event(pending, queued, result)
}

fn record_inline_result(
    pending: &mut PendingRun,
    queued: &QueuedToolCall,
    result: &ToolResultEnvelope,
) -> Result<()> {
    EventStore::open(&pending.events_path)?.append(EventPayload::ToolResult {
        turn: pending.turn,
        ts: now_timestamp()?,
        conversation: Some(pending.conversation.as_str().into()),
        tool_call_id: queued.tool_call.id.clone(),
        status: result.status.clone(),
        summary: result.summary.clone(),
        model_content: result.model_content.clone(),
        truncated: result.truncated,
        files_read: Vec::new(),
        files_changed: Vec::new(),
        commands_run: Vec::new(),
        memory_changed: None,
        structured: result.structured.clone(),
    })?;
    if result.status == "error" {
        pending.record_tool_error(&queued.tool_call.name);
    } else {
        pending.record_tool_call(&queued.tool_call.name);
    }
    Ok(())
}

fn finish_inline_event(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    result: ToolResultEnvelope,
) -> Result<PendingStep> {
    pending
        .pending_events
        .push_back(inline_tool_end(queued.tool_call.id.clone(), result));
    let event = UiEvent::ToolStart {
        id: queued.tool_call.id,
        tool: queued.tool_call.name,
        summary: queued.display_summary,
        kind: ToolKind::Simple,
    };
    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: Some(event),
    })
}

fn inline_tool_end(id: String, result: ToolResultEnvelope) -> UiEvent {
    UiEvent::ToolEnd {
        id,
        status: result.status,
        summary: result.summary,
        model_content: Some(result.model_content),
        result: result.structured,
    }
}

pub(super) fn schema(definition: &ToolDefinition) -> ToolSchema {
    ToolSchema {
        name: definition.name.clone(),
        description: definition.description.clone(),
        input_schema: definition.input_schema.clone(),
    }
}
fn schema_value(definition: &ToolDefinition) -> serde_json::Value {
    serde_json::json!({"schema":schema(definition),"read_only":definition.read_only,"risk":definition.risk,"max_result_chars":definition.max_result_chars})
}
fn definition(name: &str, description: &str, input_schema: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema,
        read_only: name == "list_agents",
        max_result_chars: 16_384,
        risk: "session".into(),
    }
}
fn required<'a>(args: &'a serde_json::Value, field: &str) -> Result<&'a str> {
    args[field]
        .as_str()
        .ok_or_else(|| Error::InvalidArgument(format!("missing tool argument '{field}'")))
}

#[cfg(test)]
mod tests {
    use super::{create_agent_input, current_agent_after_wait, normalize_create_agent_args};
    use crate::Error;
    use serde_json::json;

    #[test]
    fn list_agents_wait_falls_back_only_on_timeout() {
        let mut fallback_called = false;
        let current = current_agent_after_wait(Err(Error::AgentWaitTimeout), || {
            fallback_called = true;
            Ok("current state")
        })
        .unwrap();
        assert_eq!("current state", current);
        assert!(fallback_called);

        let mut fallback_called = false;
        let error = current_agent_after_wait::<&str>(
            Err(Error::InvalidEventStream("worker failure".into())),
            || {
                fallback_called = true;
                Ok("current state")
            },
        )
        .unwrap_err();
        assert!(matches!(error, Error::InvalidEventStream(message) if message == "worker failure"));
        assert!(!fallback_called);
    }

    #[test]
    fn create_agent_preserves_values_outside_optional_identifiers() {
        let mut args = json!({"address":"reviewer","tier":" ", "instructions":"", "tools":[]});
        normalize_create_agent_args(&mut args);
        let (address, config) = create_agent_input(&args).unwrap();
        assert_eq!("reviewer", address);
        assert_eq!(None, config.tier);
        assert_eq!(Some(""), config.instructions.as_deref());
        assert_eq!(Some(Vec::new()), config.tools);
        for args in [
            json!({"address":" "}),
            json!({"tier":"balanced"}),
            json!({"address":"reviewer","max_output_tokens":""}),
            json!({"address":"reviewer","no_agents":""}),
        ] {
            assert!(create_agent_input(&args).is_err(), "{args}");
        }
    }
}
