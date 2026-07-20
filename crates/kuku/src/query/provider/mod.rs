use crate::context::{
    assemble_context, rebuild_history_for_provider, CanonicalMessage, ContextInput,
    EnvironmentSource,
};
use crate::error::Result;
use crate::event::{EventPayload, RequestCause, RequestId, RequestScope, RequestStarted};
use crate::log::{LogLevel, LogRecord, LogScope};
use crate::notice::compute_context_headroom;
use crate::prompt::{builtin_handoff_instruction, load_prompt_template};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::tool;

mod assembly;
pub(crate) mod request;

pub(crate) use request::{LifecycleOnlyRecorder, RequestEvidenceRecorder};

use assembly::{
    append_current_turn_prefix_once, append_handoff_instruction, assembly_runtime_prefix,
    build_current_user_message, build_runtime_blocks, insert_current_turn_metadata_block,
    replace_current_user_message, should_trigger_handoff,
};

use super::helpers::{
    append_model_error, append_turn_interrupted, current_date_string, last_input_tokens,
    load_memory_sources, load_project_instruction_sources, now_timestamp, platform_label,
};
use super::tool_exec::record_plugin_hooks;
use super::types::{PendingRun, PendingStep, ResolvedRuntime, StreamingChunkState, UiEvent};

const MAX_REQUEST_LOOP: u64 = 20;

pub(super) async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
    pending.verify_workspace()?;
    ensure_resolved(&mut pending)?;
    pending.request_num += 1;
    check_loop_limit(&pending)?;

    let resolved = pending.resolved.as_ref().expect("resolved runtime exists");
    let resolved_config = resolved.config.clone();
    let registry = resolved.registry.clone();
    let existing_events = pending.event_store.read_all()?;
    let (handoff_summary, history) =
        rebuild_history_for_provider(&existing_events, &pending.conversation);
    let project_instructions = load_project_instruction_sources(
        &pending.workspace,
        pending.workspace_capability.as_deref(),
    )?;
    let (global_memory, project_memory) = load_memory_sources(
        &pending.kuku_home,
        &pending.workspace,
        pending.workspace_capability.as_deref(),
    )?;
    let platform = platform_label().to_string();
    let current_date = current_date_string();
    let model_tiers = pending.config.tier_infos();

    let catalog = if let Some(dir) = &pending.prompts_dir {
        crate::prompt::PromptCatalog::load_from_dir(dir).map_err(|e| {
            crate::error::Error::PromptRender(format!(
                "failed to load prompts from {}: {e}",
                dir.display()
            ))
        })?
    } else {
        crate::prompt::builtin_prompt_catalog()
    };

    let (catalog_text, skills_text, runtime_blocks) = build_runtime_blocks(
        &pending.workspace,
        pending.workspace_capability.as_deref(),
        pending.conversation.as_str(),
        pending.turn,
        pending.agent_registry.as_ref(),
        pending.skill_registry.as_ref(),
        pending.previous_skill_registry.as_ref(),
        &resolved_config,
        &existing_events,
        &catalog,
    )?;

    let runtime_blocks = if pending.turn == 1 {
        if let Some(ref plugin_reg) = pending.plugin_registry {
            if !plugin_reg.is_empty() {
                let pkg_names = plugin_reg.names().join(", ");
                let notice = format!(
                    "Plugins loaded: {pkg_names}. \
                     If not relevant to your current task, ignore."
                );
                let wrapper_tmpl = catalog
                    .blocks
                    .get("system-notice")
                    .map(|a| a.text.as_str())
                    .unwrap_or("<kuku_system_notice>\n{{notice_body}}\n</kuku_system_notice>");
                let wrapped = wrapper_tmpl.replace("{{notice_body}}", &notice);
                Some(match runtime_blocks {
                    Some(existing) => format!("{existing}\n\n{wrapped}"),
                    None => wrapped,
                })
            } else {
                runtime_blocks
            }
        } else {
            runtime_blocks
        }
    } else {
        runtime_blocks
    };

    let mut assembly = match assemble_context(
        ContextInput {
            environment: EnvironmentSource {
                workspace_path: pending.workspace.display().to_string(),
                platform: platform.clone(),
                current_date: current_date.clone(),
            },
            project_instructions,
            global_memory,
            project_memory,
            history,
            tools: tool::to_tool_schemas(&registry),
            model_tiers,
            runtime_blocks,
            enable_memory: true,
            agent_name: pending.conversation.root_contact().as_str().to_string(),
            agent_instructions: pending
                .agent_registry
                .as_ref()
                .and_then(|r| r.get(pending.conversation.root_contact().as_str()))
                .map(|d| d.instructions.clone())
                .or_else(|| pending.query.agent_instructions.clone())
                .or_else(|| catalog.agents.get("main").map(|a| a.text.clone()))
                .unwrap_or_default(),
            response_contract: pending.query.response_contract.clone(),
        },
        &catalog,
    ) {
        Ok(assembly) => assembly,
        Err(error) => {
            append_turn_interrupted(
                &pending.event_store,
                pending.execution_scope(),
                &pending.conversation,
                pending.turn,
                "prompt_render_error",
            )?;
            return Err(error);
        }
    };

    if let Some(catalog_text) = catalog_text {
        if !catalog_text.is_empty() {
            assembly
                .prelude_messages
                .push(CanonicalMessage::user_text(catalog_text));
        }
    }
    if let Some(skills_text) = skills_text {
        if !skills_text.is_empty() {
            assembly
                .prelude_messages
                .push(CanonicalMessage::user_text(skills_text));
        }
    }

    if let Some(prefix) = pending
        .query
        .current_turn_prefix
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        append_current_turn_prefix_once(&mut assembly.prelude_messages, prefix);
    }

    if let Some(skill) = pending.bootstrap_skill.as_ref() {
        assembly
            .prelude_messages
            .push(crate::context::CanonicalMessage::user_text(
                skill.body.clone(),
            ));
    }

    assembly.handoff_summary = handoff_summary;

    let estimated_input = last_input_tokens(&resolved_config.kind, &existing_events);
    let thinking_overhead = resolved_config.think_level.overhead_tokens();
    let headroom = compute_context_headroom(
        resolved_config
            .max_context_tokens
            .saturating_sub(thinking_overhead),
        Some(resolved_config.max_output_tokens),
        estimated_input,
    );

    let handoff_instruction = {
        let handoff_config = pending.config.handoff();
        if !pending.handoff_triggered
            && handoff_config.enabled
            && should_trigger_handoff(&headroom, handoff_config.threshold)
        {
            pending.handoff_triggered = true;
            pending.handoff_keep_turns = handoff_config.keep_turns;
            Some(if let Some(dir) = &pending.prompts_dir {
                load_prompt_template(dir, "runtime/handoff-instruction")
                    .unwrap_or_else(|_| builtin_handoff_instruction().to_string())
            } else {
                builtin_handoff_instruction().to_string()
            })
        } else {
            None
        }
    };

    let dynamic_turn_prefix = assembly_runtime_prefix(
        assembly.runtime_context.as_deref(),
        pending
            .bootstrap_skill
            .as_ref()
            .map(|skill| skill.body.as_str()),
        &catalog,
    );
    let mut current_turn_prefix = pending
        .frozen_turn_prefix
        .freeze_or_reuse(dynamic_turn_prefix);
    if let Some(instruction) = handoff_instruction {
        current_turn_prefix =
            append_handoff_instruction(current_turn_prefix, &instruction, &catalog);
        pending
            .frozen_turn_prefix
            .replace(current_turn_prefix.clone());
    }
    let current_body = pending
        .query
        .current_turn_body
        .as_deref()
        .unwrap_or(&pending.query.prompt);
    let current_input = build_current_user_message(current_turn_prefix, current_body);
    if !replace_current_user_message(
        &mut assembly.history,
        &pending.query.prompt,
        current_body,
        current_input.clone(),
    ) {
        assembly.history.push(current_input.clone());
    }

    if !pending.hook_context.is_empty() {
        let hook_text = pending.hook_context.join("\n");
        pending.hook_context.clear();
        if let Some(last_user) = assembly
            .history
            .iter_mut()
            .rev()
            .find(|m| m.role == crate::context::Role::User)
        {
            let hook_tmpl = catalog
                .blocks
                .get("hook-context")
                .map(|a| a.text.as_str())
                .unwrap_or("<kuku_hook_context>\n{{hook_text}}\n</kuku_hook_context>");
            insert_current_turn_metadata_block(
                last_user,
                hook_tmpl.replace("{{hook_text}}", &hook_text),
            );
        }
    }

    if let Some(ref plugin_reg) = pending.plugin_registry {
        let hooks = plugin_reg.hooks_for(crate::plugin::HookEvent::ModelPreRequest);
        if !hooks.is_empty() {
            let input = crate::plugin::executor::HookInput {
                event: "model.pre_request".into(),
                session_dir: pending
                    .events_path
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .into(),
                extra: serde_json::json!({ "tier": pending.query.tier }),
            };
            let sd = pending.events_path.parent().unwrap().to_path_buf();
            let ws = pending.workspace.clone();
            let results = crate::plugin::executor::execute_hooks(hooks, &input, &sd, &ws).await?;
            for r in &results {
                if let Some(ref ctx) = r.output.additional_context {
                    pending.hook_context.push(ctx.clone());
                }
            }
            record_plugin_hooks(
                &pending.events_path,
                pending.turn,
                "model.pre_request",
                &results,
            )?;
        }
    }

    let request_id = RequestId::try_new()?;
    let request_scope = RequestScope {
        execution: pending
            .query
            .execution_scope
            .clone()
            .expect("execution scope assigned at start"),
        request_id: request_id.clone(),
    };
    let _tier_name = pending
        .query
        .tier
        .clone()
        .unwrap_or_else(|| pending.config.default_tier().to_string());
    let think = resolved_config.think_level;
    let max_output = resolved_config.max_output_tokens;
    let _params = serde_json::json!({
        "max_output_tokens": max_output,
        "temperature": pending.query.temperature,
    });

    {
        pending.event_store.append(EventPayload::ContextSources {
            request: request_scope.clone(),
            turn: pending.turn,
            ts: now_timestamp()?,
            project_instruction_sources: assembly
                .project_instruction_sources
                .iter()
                .map(|source| crate::context::FileSource {
                    path: source.path.clone(),
                    hash: source.hash.clone(),
                })
                .collect(),
            memory_sources: assembly
                .memory_sources
                .iter()
                .map(|source| crate::context::FileSource {
                    path: source.path.clone(),
                    hash: source.hash.clone(),
                })
                .collect(),
        })?;
    }

    let request = crate::provider::types::ProviderRequest {
        assembly,
        catalog: &catalog,
        current_input: crate::provider::types::CanonicalPromptInput {
            parts: vec![current_input],
        },
        model: resolved_config.model.clone(),
        max_output_tokens: Some(max_output),
        temperature: pending.query.temperature,
        stream: true,
        think_level: think,
        thinking: resolved_config.thinking.clone(),
    };

    let provider_trace = Some(crate::provider::trace::ProviderTraceMetadata {
        kuku_home: pending.kuku_home.clone(),
        session_id: pending.session_id.clone(),
        turn: pending.turn,
        request_id: request_id.as_str().to_string(),
    });

    let mut lead_events = Vec::new();
    let provider_name = resolved_config.kind.as_str().to_string();
    let model_name = resolved_config.model.clone();
    emit_runtime_log(
        &mut pending,
        LogLevel::Info,
        "runtime.model_request",
        format!("requesting {provider_name} model {model_name}"),
        Some(serde_json::json!({
            "provider": provider_name,
            "model": model_name,
            "request_id": request_id.as_str(),
        })),
    )?;
    lead_events.extend(pending.pending_events.drain(..));
    lead_events.push(UiEvent::ModelRequest {
        model: model_name,
        provider: provider_name,
    });
    if pending.request_num == 1 {
        lead_events.push(UiEvent::TurnStart { turn: pending.turn });
    }

    let cause = match pending.previous_request_id.clone() {
        Some(parent_request_id) => RequestCause::ToolContinuation { parent_request_id },
        None => pending
            .query
            .initial_request_cause
            .clone()
            .unwrap_or(RequestCause::UserSubmission),
    };
    let (request_started, provider_result) = request::begin_provider_request(
        pending.request_evidence_recorder.as_ref(),
        RequestStarted {
            scope: request_scope.clone(),
            cause,
            provider: request::provider_fact(&resolved_config.kind),
            model: resolved_config.model.clone(),
            started_at: now_timestamp()?,
        },
        crate::provider::stream_provider(&resolved_config, &request, provider_trace),
    )
    .await?;
    pending.previous_request_id = Some(request_id.clone());

    let handoff_active = pending.handoff_triggered;
    match provider_result {
        Ok(stream) => {
            let conversation = pending.conversation.clone();
            Ok(PendingStep::Streaming(Box::new(StreamingChunkState {
                pending,
                conversation,
                request: request_scope,
                request_started,
                stream,
                accumulated_text: String::new(),
                accumulated_thinking: String::new(),
                stop_reason: None,
                tool_calls: Vec::new(),
                tool_arg_buffers: Vec::new(),
                provider_request_id: None,
                usage: None,
                lead_events,
                handoff_detector: if handoff_active {
                    Some(super::handoff::HandoffDetector::new())
                } else {
                    None
                },
                thinking_start: None,
                thinking_duration_ms: 0,
            })))
        }
        Err(failure)
            if matches!(
                failure.kind,
                crate::provider::types::ProviderFailureKind::ContextTooLarge
            ) =>
        {
            let user_input = existing_events
                .iter()
                .rev()
                .find_map(|e| match &e.payload {
                    EventPayload::MessageUser {
                        conversation, text, ..
                    } if conversation == pending.conversation.as_str() => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            pending.event_store.append(EventPayload::Handoff {
                execution: request_scope.execution.clone(),
                turn: pending.turn,
                ts: now_timestamp()?,
                request_id: request_id.as_str().to_string(),
                summary: user_input,
                keep_turns: pending.handoff_keep_turns,
            })?;
            pending.event_store.append(EventPayload::ModelError {
                request: request_scope.clone(),
                turn: pending.turn,
                ts: now_timestamp()?,
                kind: "context_too_large".to_string(),
                message: failure.message.clone(),
            })?;
            append_turn_interrupted(
                &pending.event_store,
                pending.execution_scope(),
                &pending.conversation,
                pending.turn,
                "context_too_large",
            )?;
            pending
                .request_evidence_recorder
                .record_failed(request::failed(
                    request_scope,
                    request_started,
                    failure.provider_request_id.clone(),
                    None,
                    failure.kind,
                    failure.message.clone(),
                ))?;
            Ok(pending_failure_step(
                pending,
                lead_events,
                crate::error::Error::Provider {
                    kind: failure.kind,
                    message: failure.message,
                    provider: Some(resolved_config.kind.as_str().to_string()),
                    model: Some(resolved_config.model.clone()),
                },
            ))
        }
        Err(failure) => {
            append_model_error(
                &pending.event_store,
                request_scope.clone(),
                pending.turn,
                failure.kind.as_event_kind(),
                &failure.message,
            )?;
            append_turn_interrupted(
                &pending.event_store,
                pending.execution_scope(),
                &pending.conversation,
                pending.turn,
                failure.kind.as_event_kind(),
            )?;
            pending
                .request_evidence_recorder
                .record_failed(request::failed(
                    request_scope,
                    request_started,
                    failure.provider_request_id.clone(),
                    None,
                    failure.kind,
                    failure.message.clone(),
                ))?;
            Ok(pending_failure_step(
                pending,
                lead_events,
                crate::error::Error::Provider {
                    kind: failure.kind,
                    message: failure.message,
                    provider: Some(resolved_config.kind.as_str().to_string()),
                    model: Some(resolved_config.model.clone()),
                },
            ))
        }
    }
}

trait ProviderFailureKindEventName {
    fn as_event_kind(&self) -> &'static str;
}

impl ProviderFailureKindEventName for crate::provider::types::ProviderFailureKind {
    fn as_event_kind(&self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::RateLimited => "rate_limited",
            Self::ContextTooLarge => "context_too_large",
            Self::InvalidRequest => "invalid_request",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::Transport => "transport",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
        }
    }
}

fn pending_failure_step(
    mut pending: PendingRun,
    lead_events: Vec<UiEvent>,
    error: crate::error::Error,
) -> PendingStep {
    pending.pending_events.extend(lead_events.into_iter().rev());
    pending.flush_runtime_logs();
    pending.pending_error = Some(error);
    PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: None,
    }
}

pub(super) fn emit_runtime_log(
    pending: &mut PendingRun,
    level: LogLevel,
    kind: impl Into<String>,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Result<()> {
    let mut record = LogRecord::new(now_timestamp()?, level, LogScope::Runtime);
    record.kind = kind.into();
    record.message = message.into();
    record.session_id = Some(pending.session_id.clone());
    record.run_id = Some(pending.execution_scope().run_id.to_string());
    record.workspace = Some(pending.workspace.display().to_string());
    record.turn = Some(pending.turn);
    record.data = data;
    pending.pending_events.push_back(UiEvent::Log { record });
    Ok(())
}

fn check_loop_limit(pending: &PendingRun) -> Result<()> {
    if pending.request_num > MAX_REQUEST_LOOP {
        let provider_name = pending
            .resolved
            .as_ref()
            .map(|r| r.config.kind.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let model = pending
            .resolved
            .as_ref()
            .map(|r| r.config.model.clone())
            .unwrap_or_else(|| "unknown".to_string());
        append_turn_interrupted(
            &pending.event_store,
            pending.execution_scope(),
            &pending.conversation,
            pending.turn,
            "loop_limit",
        )?;
        return Err(crate::error::Error::Provider {
            kind: crate::provider::types::ProviderFailureKind::Unknown,
            message: "tool loop exceeded maximum provider requests".to_string(),
            provider: Some(provider_name),
            model: Some(model),
        });
    }
    Ok(())
}

pub(super) fn ensure_resolved(pending: &mut PendingRun) -> Result<()> {
    if pending.resolved.is_some() {
        return Ok(());
    }

    let config = match resolve_config(ResolveConfigInput {
        provider: pending.query.provider,
        model: pending.query.model.clone(),
        tier: pending.query.tier.clone(),
        base_url: pending.query.base_url.clone(),
        api_key: pending.query.api_key.clone(),
        max_output_tokens: pending.query.max_output_tokens,
        config: Some((*pending.config).clone()),
    }) {
        Ok(config) => config,
        Err(error) => {
            append_turn_interrupted(
                &pending.event_store,
                pending.execution_scope(),
                &pending.conversation,
                pending.turn,
                "missing_config",
            )?;
            return Err(error);
        }
    };

    let registry = if let Some(ref overridden) = pending.tool_registry_override {
        overridden.clone()
    } else {
        tool::builtin_registry(!pending.query.disable_agents, !pending.query.disable_skills)
    };
    pending.resolved = Some(ResolvedRuntime { config, registry });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::MessageBlock;

    fn message_text(message: &CanonicalMessage) -> &str {
        match &message.blocks[0] {
            MessageBlock::Text(text) => text,
            _ => panic!("expected text block"),
        }
    }

    #[test]
    fn handoff_trigger_requires_known_token_headroom() {
        let headroom = compute_context_headroom(200_000, Some(64_000), None);

        assert!(!should_trigger_handoff(&headroom, 0.7));
    }

    #[test]
    fn handoff_trigger_uses_known_token_headroom() {
        let headroom = compute_context_headroom(200_000, Some(64_000), Some(125_000));

        assert!(should_trigger_handoff(&headroom, 0.7));
    }

    #[test]
    fn delegated_body_replacement_prefers_current_wrapped_message() {
        let raw = "same text";
        let wrapped = "<kuku_delegated_prompt>\nsame text\n</kuku_delegated_prompt>";
        let replacement = CanonicalMessage::user_text("provider body");
        let mut history = vec![
            CanonicalMessage::user_text(raw),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer".to_string())]),
            CanonicalMessage::user_text(wrapped),
        ];

        assert!(replace_current_user_message(
            &mut history,
            raw,
            wrapped,
            replacement
        ));

        assert_eq!(message_text(&history[0]), raw);
        assert_eq!(message_text(&history[2]), "provider body");
    }

    #[test]
    fn current_turn_prefix_is_appended_once_to_restored_prelude() {
        let prefix = "You are a code and document reviewer";
        let mut missing = vec![CanonicalMessage::user_text("old snapshot")];
        append_current_turn_prefix_once(&mut missing, prefix);
        assert_eq!(missing.len(), 2);
        assert_eq!(message_text(&missing[1]), prefix);

        append_current_turn_prefix_once(&mut missing, prefix);
        assert_eq!(missing.len(), 2);

        let mut existing = vec![CanonicalMessage::user_text(format!(
            "before {prefix} after"
        ))];
        append_current_turn_prefix_once(&mut existing, prefix);
        assert_eq!(existing.len(), 1);
    }
}
