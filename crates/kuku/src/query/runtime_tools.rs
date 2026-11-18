pub(crate) mod output;
pub(crate) mod values;

use crate::context::ToolSchema;
use crate::event::{EventPayload, EventStore};
use crate::permission::{AuthorizationDecision, GateDecisionKind};
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, Result};

use super::helpers::{
    append_permission_decision, append_permission_request, now_timestamp, permission_candidate,
    permission_rule,
};
use super::run::find_tool_definition;
use super::tool_exec::{execute_tool_call, run_tool_pre_hooks};
use super::types::{
    PendingPermission, PendingRun, PendingStep, PermissionChoice, PermissionRequest,
    QueuedToolCall, SlotEvent, ToolKind, UiEvent,
};

pub(super) async fn advance(
    mut pending: PendingRun,
    mut queued: QueuedToolCall,
    event_tx: tokio::sync::mpsc::Sender<(String, SlotEvent)>,
    active: usize,
) -> Result<PendingStep> {
    super::provider::ensure_resolved(&mut pending)?;
    let Some(definition) = find_tool_definition(&pending, &queued.tool_call.name).cloned() else {
        let result = ToolResultEnvelope::error(
            "unknown tool",
            format!("unknown tool: {}", queued.tool_call.name),
        );
        return finish_inline(pending, queued, result);
    };
    match prepare(&mut pending, &mut queued, &definition).await {
        Ok(Some(reason)) => {
            return finish_inline(
                pending,
                queued,
                ToolResultEnvelope::blocked(&reason, &reason),
            )
        }
        Err(error) => {
            return finish_inline(
                pending,
                queued,
                ToolResultEnvelope::error("tool preparation failed", error.to_string()),
            )
        }
        Ok(None) => {}
    }
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
    let candidate = permission_candidate(
        &pending.kuku_home,
        &pending.workspace,
        &queued.tool_call.name,
        &queued.tool_call.args,
    );
    let workspace = if queued.tool_call.name == "run_command" {
        queued.tool_call.args["cwd"]
            .as_str()
            .map_or_else(|| pending.workspace.clone(), std::path::PathBuf::from)
    } else {
        pending.workspace.clone()
    };
    let action = crate::permission::ToolAuthorization {
        session_id: pending.session_id.clone(),
        agent: pending.conversation.as_str().into(),
        source: pending.query.input_source.clone(),
        tool_call_id: queued.tool_call.id.clone(),
        tool: schema(&definition),
        args: queued.tool_call.args.clone(),
        workspace,
    };
    let policy = crate::permission::load_project_policy(&pending.policy_path)?;
    let decision = match authorize(
        &policy,
        &definition,
        &candidate,
        pending
            .query
            .authorizer
            .as_deref()
            .expect("session query has host authority"),
        &action,
    ) {
        Ok(decision) => decision,
        Err(error) => {
            return finish_inline(
                pending,
                queued,
                ToolResultEnvelope::error(
                    "host authorization failed; action was not dispatched",
                    error.to_string(),
                ),
            )
        }
    };
    match decision {
        AuthorizationDecision::Defer => {
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
                source: "host".into(),
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
        AuthorizationDecision::Deny => {
            append_permission_decision(
                &pending.events_path,
                pending.turn,
                &queued.tool_call.id,
                PermissionChoice::Deny,
                "host",
                &candidate,
            )?;
            pending.record_tool_denied(&queued.tool_call.name);
            finish_inline(
                pending,
                queued,
                ToolResultEnvelope::blocked(
                    "host denied tool action",
                    "the action was not dispatched",
                ),
            )
        }
        AuthorizationDecision::Allow => {
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
    if let Some(result) = super::workspace_script::changed_file_target(
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
        let slot = super::adapters::spawn_tool(registered, action, output, event_tx);
        pending.record_tool_call(&queued.tool_call.name);
        let event = UiEvent::ToolStart {
            id: queued.tool_call.id,
            tool: queued.tool_call.name,
            summary: queued.display_summary,
            kind: ToolKind::Simple,
        };
        return Ok(PendingStep::Pending {
            pending: Box::new(pending),
            slot: Some(slot),
            event: Some(event),
        });
    }
    if queued.tool_call.name == "workspace_script" {
        mark_dispatched(&pending, &queued)?;
        let slot = super::workspace_script::spawn(&pending, &queued, event_tx)?;
        pending.record_tool_call(&queued.tool_call.name);
        let event = UiEvent::ToolStart {
            id: queued.tool_call.id,
            tool: queued.tool_call.name,
            summary: queued.display_summary,
            kind: ToolKind::Simple,
        };
        return Ok(PendingStep::Pending {
            pending: Box::new(pending),
            slot: Some(slot),
            event: Some(event),
        });
    }
    let prior_events = crate::context::runtime::read_agent_events(
        &pending.events_path,
        pending.conversation.as_str(),
        pending.query.runtime_limits.max_request_bytes,
    )?;
    let output = if matches!(queued.tool_call.name.as_str(), "run_command" | "fetch_url") {
        match output_spec(&pending, &queued.tool_call.id) {
            Ok(output) => Some(output),
            Err(error) => return output_admission_failed(pending, queued, error),
        }
    } else {
        None
    };
    mark_dispatched(&pending, &queued)?;
    let (slot, kind) = super::slots::dispatch_tool_slot(super::slots::SlotDispatchArgs {
        capacity: runtime_session(&pending)?.workspace_execution_host(),
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
    pending.record_tool_call(&queued.tool_call.name);
    let event = UiEvent::ToolStart {
        id: queued.tool_call.id,
        tool: queued.tool_call.name,
        summary: queued.display_summary,
        kind,
    };
    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: Some(slot),
        event: Some(event),
    })
}

pub(super) fn should_yield(pending: &PendingRun) -> Result<bool> {
    let mut after = pending.turn.saturating_sub(1);
    loop {
        let events = EventStore::read_page(&pending.events_path, after, 64)?;
        if events.is_empty() {
            return Ok(false);
        }
        for event in events {
            after = event.id;
            if matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.operation" && value["record"]["operation"] == "question_requested" && value["record"]["address"] == pending.conversation.as_str() && value["record"]["request"]["turn"] == pending.turn)
            {
                return Ok(true);
            }
        }
    }
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

pub(super) fn requires_workspace_ordering(pending: &PendingRun, name: &str) -> bool {
    pending
        .query
        .tool_adapters
        .get(name)
        .is_some_and(|tool| tool.spec.access != crate::ToolAccess::Read)
        || super::slots::requires_workspace_ordering(name)
}

pub(super) fn is_inline(name: &str) -> bool {
    matches!(
        name,
        "create_agent"
            | "send_message"
            | "list_agents"
            | "ask_user"
            | "read_output"
            | "save_value"
            | "list_values"
    )
}

pub(super) fn reject(pending: PendingRun, queued: QueuedToolCall) -> Result<PendingStep> {
    if queued.tool_call.name == "workspace_script" {
        if let Some(result) = super::workspace_script::declined(&pending, &queued.tool_call.id)? {
            return finish_inline(pending, queued, result);
        }
    }
    finish_inline(
        pending,
        queued,
        ToolResultEnvelope::blocked("host denied tool action", "the action was not dispatched"),
    )
}

fn output_spec(pending: &PendingRun, id: &str) -> Result<super::slots::RetainedOutputSpec> {
    let agent = pending.conversation.as_str();
    let output_id = crate::session::runtime::command_output_id(agent, pending.turn, id);
    let mut retention = runtime_session(pending)?.reserve_output(agent, &output_id)?;
    retention.discard_on_drop();
    Ok(super::slots::RetainedOutputSpec {
        dir: pending
            .events_path
            .parent()
            .expect("session events has a parent")
            .join("outputs"),
        id: output_id,
        limits: crate::tool::builtin::run_command::output::OutputLimits {
            max_output_bytes: retention.max_bytes(),
            max_preview_bytes: pending.query.runtime_limits.max_preview_bytes,
        },
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

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    vec![
        crate::tool::workspace_script::definition(),
        output::definition(),
        values::save_definition(),
        values::list_definition(),
        definition("ask_user", "Persist a question and yield after the current tool batch. Wait for the host to reply to the returned request identity before continuing.", serde_json::json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"]})),
        definition(
            "create_agent",
            "Create an independent Agent. Send its task with send_message after creation.",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "address": {"type": "string", "description": "Stable Session-local name."},
                    "instructions": {"type": "string", "description": "Standing role or constraints for this Agent."},
                    "tools": {"type": "array", "items": {"type":"string"}, "description":"Exact tool names. Omit to use defaults. An empty array means no tools."},
                    "preset": {"type": "string", "description": "Configured Agent preset."},
                    "tier": {"type": "string", "description": "Configured model tier from <kuku_models>. Omit to use the preset or Session default."},
                    "workspace": {"type": "string", "description": "Working directory when different from the Session workspace."}
                },
                "required": ["address"]
            }),
        ),
        definition("send_message", "Deliver a message and any selected value references to an existing agent. Returns a durable receipt; query or receive its later result separately.", serde_json::json!({"type":"object","properties":{"to":{"type":"string"},"message":{"type":"string"},"wake":{"type":"boolean","description":"Defaults true. False only enqueues the message."},"values":values::handoff_schema()},"required":["to","message"]})),
        definition("list_agents", "List compact Agent states. When you need a delegated result, pass wait_for with the recipient address once. Waits up to 30 seconds for its current execution and returns its settled reply or current state.", serde_json::json!({"type":"object","properties":{"wait_for":{"type":"string","minLength":1}}})),
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
        "read_output" => {
            let permits = session.workspace_execution_host().operations();
            let permit = permits
                .acquire_owned()
                .await
                .map_err(|_| Error::InvalidArgument("workspace capacity closed".into()))?;
            let agent = agent.to_owned();
            let args = call.args.clone();
            return tokio::task::spawn_blocking(move || {
                let _permit = permit;
                output::execute(&session, &agent, &args)
            })
            .await
            .map_err(|err| Error::InvalidEventStream(err.to_string()))?;
        }
        "save_value" => return values::save(&session, agent, &call.args).await,
        "list_values" => return values::list(&session, agent, &call.args),
        "create_agent" => {
            let (address, config) = create_agent_input(&call.args)?;
            serde_json::to_value(session.create_agent(&key, &address, config)?)?
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
                serde_json::json!([agent_listing(session.wait_for_agent(address).await?, true)])
            } else {
                serde_json::Value::Array(
                    session
                        .agents()
                        .into_iter()
                        .map(|agent| agent_listing(agent, false))
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
            )))
        }
    };
    Ok(ToolResultEnvelope::ok(
        "session operation committed",
        serde_json::to_string(&value)?,
        value,
    ))
}

fn runtime_session(pending: &PendingRun) -> Result<crate::Session> {
    let execution = pending
        .query
        .runtime_execution
        .expect("session runtime owns this execution");
    super::provider::runtime_session(pending, execution)
}

fn agent_listing(agent: crate::AgentSnapshot, include_reply: bool) -> serde_json::Value {
    let mut item = serde_json::json!({"address": agent.address, "status": agent.status});
    if agent.pending_inputs > 0 {
        item["pending_inputs"] = agent.pending_inputs.into();
    }
    if agent.permission.is_some() {
        item["needs_approval"] = true.into();
    }
    if agent.question.is_some() {
        item["needs_reply"] = true.into();
    }
    if let Some(error) = agent.last_error {
        item["last_error"] = error.into();
    } else if agent.status == crate::AgentStatus::Idle {
        if let Some(output) = agent.last_output {
            if include_reply {
                item["last_output"] = output.into();
            } else {
                item["reply_available"] = true.into();
            }
        }
    }
    item
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentInput {
    address: String,
    instructions: Option<String>,
    tools: Option<Vec<String>>,
    preset: Option<String>,
    tier: Option<String>,
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
            workspace: input.workspace,
            ..Default::default()
        },
    ))
}

fn normalize_create_agent_args(args: &mut serde_json::Value) {
    let Some(values) = args.as_object_mut() else {
        return;
    };
    for field in ["tier", "preset", "workspace"] {
        if values
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.trim().is_empty())
        {
            values.remove(field);
        }
    }
}

pub(super) fn validate_prepared(
    pending: &PendingRun,
    call: &crate::provider::types::ProviderToolCall,
) -> Result<()> {
    let definition = find_tool_definition(pending, &call.name).ok_or_else(|| {
        Error::InvalidArgument(format!("tool '{}' is no longer available", call.name))
    })?;
    let prepared = prepared_record(pending, &call.id)?.ok_or_else(|| {
        Error::InvalidEventStream("pending action has no durable prepared arguments".into())
    })?;
    if prepared["args"] != call.args || prepared["definition"] != schema_value(definition) {
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
    if let Some(prepared) = prepared_record(pending, &queued.tool_call.id)? {
        if prepared["definition"] != schema_value(definition) {
            return Err(Error::InvalidArgument(
                "prepared tool definition changed before dispatch".into(),
            ));
        }
        queued.tool_call.args = prepared["args"].clone();
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
    super::workspace_script::prepare_file_target(
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
        )
        .map_err(|result| Error::InvalidArgument(result.model_content))?;
    }
    let agent = pending.conversation.as_str();
    let turn = pending.turn;
    match queued.tool_call.name.as_str() {
        "read_output" => output::prepare(
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
    EventStore::open(&pending.events_path)?.append(EventPayload::Unknown(serde_json::json!({"kind":"runtime.tool_prepared","ts":now_timestamp()?,"agent":pending.conversation.as_str(),"turn":pending.turn,"tool_call_id":queued.tool_call.id,"args":queued.tool_call.args,"definition":schema_value(definition)})))?;
    Ok(None)
}

fn prepared_record(pending: &PendingRun, id: &str) -> Result<Option<serde_json::Value>> {
    let mut after = pending.turn.saturating_sub(1);
    loop {
        let events = EventStore::read_page(&pending.events_path, after, 64)?;
        if events.is_empty() {
            return Ok(None);
        }
        for event in events {
            after = event.id;
            if let EventPayload::Unknown(value) = event.payload {
                if value["kind"] == "runtime.tool_prepared"
                    && value["agent"] == pending.conversation.as_str()
                    && value["turn"] == pending.turn
                    && value["tool_call_id"] == id
                {
                    return Ok(Some(value));
                }
            }
        }
    }
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
    )
}

fn finish_inline(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    result: ToolResultEnvelope,
) -> Result<PendingStep> {
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
    pending.record_tool_call(&queued.tool_call.name);
    finish_inline_event(pending, queued, result)
}

fn finish_inline_event(
    mut pending: PendingRun,
    queued: QueuedToolCall,
    result: ToolResultEnvelope,
) -> Result<PendingStep> {
    pending.pending_events.push_back(UiEvent::ToolEnd {
        id: queued.tool_call.id.clone(),
        status: result.status,
        summary: result.summary,
        model_content: Some(result.model_content),
        result: result.structured,
    });
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

pub(super) fn authorize(
    policy: &crate::permission::PermissionPolicy,
    definition: &ToolDefinition,
    candidate: &str,
    authority: &dyn crate::permission::ToolAuthorizer,
    action: &crate::permission::ToolAuthorization,
) -> Result<AuthorizationDecision> {
    let guard = crate::permission::decide_tool_call(
        &definition.name,
        &definition.risk,
        candidate,
        policy,
        &[],
    );
    if guard.kind == GateDecisionKind::Deny {
        return Ok(AuthorizationDecision::Deny);
    }
    authority.authorize(action)
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
    use super::{create_agent_input, normalize_create_agent_args};
    use serde_json::json;

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
