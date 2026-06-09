use crate::context::{
    assemble_context, rebuild_history, restore_prompt_snapshot, AgentRegistryProvenance,
    ContextInput, EnvironmentSource, PluginRegistryProvenance, PromptCapabilityMetadata,
    PromptRendererIdentity, SkillRegistryProvenance, ToolRegistryProvenance,
};
use crate::conversation::address::ConversationAddress;
use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::log::{LogLevel, LogRecord, LogScope};
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_body, NoticeAssemblyInput,
};
use crate::prompt::{builtin_handoff_instruction, load_prompt_template};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::tool;

use super::helpers::{
    append_model_error, append_turn_interrupted, current_date_string, last_input_tokens,
    load_memory_sources, load_project_instruction_sources, now_timestamp, platform_label,
};
use super::tool_exec::record_plugin_hooks;
use super::types::{PendingRun, PendingStep, ResolvedRuntime, StreamingChunkState, UiEvent};

const MAX_REQUEST_LOOP: u64 = 20;

pub(super) async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
    ensure_resolved(&mut pending)?;
    pending.request_num += 1;
    check_loop_limit(&pending)?;

    let resolved = pending.resolved.as_ref().expect("resolved runtime exists");
    let resolved_config = resolved.config.clone();
    let registry = resolved.registry.clone();
    let existing_events = EventStore::replay(&pending.events_path)?;
    let (handoff_summary, history) = rebuild_history(&existing_events, &pending.conversation);
    let project_instructions = load_project_instruction_sources(&pending.workspace)?;
    let (global_memory, project_memory) =
        load_memory_sources(&pending.kuku_home, &pending.workspace)?;
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

    let (runtime_blocks, _notice_snapshots) = build_runtime_blocks(
        &pending.workspace,
        pending.conversation.as_str(),
        pending.turn,
        pending.agent_registry.as_ref(),
        pending.skill_registry.as_ref(),
        pending.previous_skill_registry.as_ref(),
        &resolved_config,
        &existing_events,
    )?;

    let runtime_blocks = if pending.turn == 1 {
        if let Some(ref plugin_reg) = pending.plugin_registry {
            if !plugin_reg.is_empty() {
                let pkg_names = plugin_reg.names().join(", ");
                let notice = format!(
                    "Plugins loaded: {pkg_names}. \
                     If not relevant to your current task, ignore."
                );
                let wrapped = format!("<kuku_system_notice>\n{notice}\n</kuku_system_notice>");
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
        },
        &catalog,
    ) {
        Ok(assembly) => assembly,
        Err(error) => {
            let request_id = format!("req_{}", pending.request_num);
            append_model_error(
                &pending.events_path,
                pending.turn,
                request_id,
                "prompt_render",
                &error.to_string(),
            )?;
            append_turn_interrupted(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                "prompt_render_error",
            )?;
            return Err(error);
        }
    };

    let frozen = restore_prompt_snapshot(&existing_events, pending.conversation.as_str());
    let is_first_request = frozen.is_none();
    if let Some(frozen) = frozen {
        assembly.prelude_messages = frozen;
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

    {
        let handoff_config = pending.config.handoff();
        if handoff_config.enabled {
            let budget = (headroom.max_context_tokens
                - headroom.reserved_output_tokens
                - headroom.reserved_margin_tokens) as f64;
            if budget > 0.0 {
                let remaining = headroom.remaining_input_tokens.unwrap_or(0) as f64;
                let used_ratio = 1.0 - (remaining / budget);
                if used_ratio >= handoff_config.threshold {
                    pending.handoff_triggered = true;
                    pending.handoff_keep_turns = handoff_config.keep_turns;
                    let instruction = if let Some(dir) = &pending.prompts_dir {
                        load_prompt_template(dir, "handoff-instruction")
                            .unwrap_or_else(|_| builtin_handoff_instruction().to_string())
                    } else {
                        builtin_handoff_instruction().to_string()
                    };
                    let rt = assembly.runtime_context.get_or_insert_with(String::new);
                    rt.push_str("\n\n");
                    rt.push_str(&instruction);
                }
            }
        }
    }

    let prelude_snapshot = assembly.snapshot_prelude();
    let current_input = build_current_input_frame(
        assembly_runtime_prefix(
            assembly.runtime_context.as_deref(),
            pending
                .bootstrap_skill
                .as_ref()
                .map(|skill| skill.body.as_str()),
        ),
        &pending.query.prompt,
    );
    assembly.history.push(current_input.clone());

    if !pending.hook_context.is_empty() {
        let hook_text = pending.hook_context.join("\n");
        pending.hook_context.clear();
        if let Some(last_user) = assembly
            .history
            .iter_mut()
            .rev()
            .find(|m| m.role == crate::context::Role::User)
        {
            last_user
                .blocks
                .push(crate::context::MessageBlock::Text(format!(
                    "\n\n<hook_context>\n{hook_text}\n</hook_context>"
                )));
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

    let request_id = format!("req_{}", pending.request_num);
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
        let mut store = EventStore::open(&pending.events_path)?;
        if is_first_request {
            let tool_registry = ToolRegistryProvenance {
                hash: format!("count:{}", assembly.tools.len()),
                names: assembly
                    .tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect(),
                tool_count: assembly.tools.len(),
            };
            let agent_registry =
                pending
                    .agent_registry
                    .as_ref()
                    .map(|registry| AgentRegistryProvenance {
                        hash: registry.hash().to_string(),
                        names: registry.names().to_vec(),
                    });
            let skill_registry =
                pending
                    .skill_registry
                    .as_ref()
                    .map(|registry| SkillRegistryProvenance {
                        hash: registry.hash().to_string(),
                        names: registry.names().to_vec(),
                    });
            let plugin_registry =
                pending
                    .plugin_registry
                    .as_ref()
                    .map(|registry| PluginRegistryProvenance {
                        hash: format!("count:{}", registry.names().len()),
                        names: registry.names().to_vec(),
                        count: registry.names().len(),
                    });
            store.append(EventPayload::PromptSnapshot {
                ts: now_timestamp()?,
                conversation: pending.conversation.as_str().to_string(),
                binding_id: pending
                    .agent_binding_id
                    .clone()
                    .unwrap_or_else(|| pending.conversation.as_str().to_string()),
                snapshot_id: format!(
                    "{}:{}:{}",
                    pending.conversation.as_str(),
                    pending.turn,
                    pending.request_num
                ),
                turn: pending.turn,
                messages: prelude_snapshot,
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
                prompt_asset_sources: assembly.prompt_asset_sources.clone(),
                skills: pending
                    .skill_registry
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?
                    .unwrap_or_else(|| serde_json::json!({})),
                bootstrap_loaded: pending
                    .bootstrap_skill
                    .as_ref()
                    .and_then(|skill| skill.name.clone())
                    .into_iter()
                    .collect(),
                provider: resolved_config.kind.as_str().to_string(),
                model: resolved_config.model.clone(),
                renderer: PromptRendererIdentity {
                    provider: resolved_config.kind.as_str().to_string(),
                    renderer: resolved_config.kind.as_str().to_string(),
                },
                tool_registry: Box::new(tool_registry),
                agent_registry,
                skill_registry: Box::new(skill_registry),
                plugin_registry: Box::new(plugin_registry),
                capabilities: PromptCapabilityMetadata {
                    context_budget_tier: match headroom.tier {
                        crate::notice::types::ContextBudgetTier::Tight => "tight",
                        crate::notice::types::ContextBudgetTier::Normal => "normal",
                        crate::notice::types::ContextBudgetTier::Roomy => "roomy",
                    }
                    .to_string(),
                    max_context_tokens: Some(headroom.max_context_tokens),
                    remaining_input_tokens: headroom.remaining_input_tokens,
                },
            })?;
        }
        store.append(EventPayload::ContextSources {
            turn: pending.turn,
            ts: now_timestamp()?,
            request_id: request_id.clone(),
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
            "request_id": request_id,
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

    let handoff_active = pending.handoff_triggered;
    match crate::provider::stream_provider(&resolved_config, &request).await {
        Ok(stream) => {
            let conversation = pending.conversation.clone();
            Ok(PendingStep::Streaming(Box::new(StreamingChunkState {
                pending,
                conversation,
                request_id,
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
                    EventPayload::UserInput { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let mut store = EventStore::open(&pending.events_path)?;
            store.append(EventPayload::Handoff {
                turn: pending.turn,
                ts: now_timestamp()?,
                request_id: request_id.clone(),
                summary: user_input,
                keep_turns: pending.handoff_keep_turns,
            })?;
            store.append(EventPayload::ModelError {
                turn: pending.turn,
                ts: now_timestamp()?,
                request_id: request_id.clone(),
                kind: "context_too_large".to_string(),
                message: failure.message.clone(),
            })?;
            drop(store);
            append_turn_interrupted(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                "context_too_large",
            )?;
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
                &pending.events_path,
                pending.turn,
                request_id,
                failure.kind.as_event_kind(),
                &failure.message,
            )?;
            append_turn_interrupted(
                &pending.events_path,
                &pending.conversation,
                pending.turn,
                failure.kind.as_event_kind(),
            )?;
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
    record.run_id = Some(pending.session_id.clone());
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
        append_model_error(
            &pending.events_path,
            pending.turn,
            format!("req_{}", pending.request_num),
            "loop_limit",
            "tool loop exceeded maximum provider requests",
        )?;
        append_turn_interrupted(
            &pending.events_path,
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

#[allow(clippy::too_many_arguments)]
fn build_runtime_blocks(
    workspace: &std::path::Path,
    conversation: &str,
    turn: u64,
    agent_registry: Option<&crate::agent::registry::AgentRegistry>,
    skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    previous_skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    resolved_config: &crate::provider::types::ResolvedProvider,
    existing_events: &[crate::event::StoredEvent],
) -> Result<(Option<String>, Vec<crate::event::types::ContextMessage>)> {
    let estimated_input = last_input_tokens(&resolved_config.kind, existing_events);
    let thinking_overhead = resolved_config.think_level.overhead_tokens();
    let context_headroom = compute_context_headroom(
        resolved_config
            .max_context_tokens
            .saturating_sub(thinking_overhead),
        Some(resolved_config.max_output_tokens),
        estimated_input,
    );

    let mut parts: Vec<String> = Vec::new();
    let mut notice_bodies: Vec<String> = Vec::new();
    let mut notice_snapshots: Vec<crate::event::types::ContextMessage> = Vec::new();

    if let Some(agent_registry) = agent_registry {
        if let Some(catalog_text) = crate::agent::catalog::render_agent_catalog(agent_registry) {
            parts.push(catalog_text);
        }
    }

    if let Some(skill_reg) = skill_registry {
        let loaded_skill_names =
            crate::skill::session::loaded_skill_names(existing_events, conversation);
        let skill_changes = if turn > 1 {
            previous_skill_registry.and_then(|previous_skill_registry| {
                crate::skill::registry::detect_skill_changes(previous_skill_registry, skill_reg)
            })
        } else {
            None
        };

        if let Some(catalog_text) = crate::skill::catalog::render_skill_catalog(
            skill_reg,
            &loaded_skill_names,
            skill_changes.as_ref(),
        ) {
            parts.push(catalog_text);
        }
    }

    if turn > 1 {
        let conversation =
            ConversationAddress::parse(conversation).unwrap_or(ConversationAddress::MAIN);
        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace,
            events: existing_events,
            context_budget_tier: context_headroom.tier,
            conversation: &conversation,
            agent_registry,
        });
        for notice in &notices {
            if let Some(body) = render_notice_body(notice) {
                notice_bodies.push(body);
            }
        }
        notice_snapshots = notices
            .iter()
            .filter_map(|n| {
                render_notice_body(n).map(|content| crate::event::types::ContextMessage {
                    role: "user".to_string(),
                    content,
                })
            })
            .collect();
    }

    if !notice_bodies.is_empty() {
        let merged = notice_bodies.join("\n\n");
        parts.push(format!(
            "<kuku_system_notice>\n{merged}\n</kuku_system_notice>"
        ));
    }

    let runtime_blocks = if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    };

    Ok((runtime_blocks, notice_snapshots))
}

fn assembly_runtime_prefix(
    runtime_context: Option<&str>,
    skill_body: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(runtime_context) = runtime_context.filter(|value| !value.is_empty()) {
        parts.push(format!(
            "<runtime.notices>{runtime_context}</runtime.notices>"
        ));
    }
    if let Some(skill_body) = skill_body.filter(|value| !value.is_empty()) {
        parts.push(format!(
            "<conversation.inbox>{skill_body}</conversation.inbox>"
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn build_current_input_frame(
    prefix: Option<String>,
    prompt: &str,
) -> crate::context::CanonicalMessage {
    let mut text = String::from("<kuku_turn_frame>\n");
    if let Some(prefix) = prefix {
        text.push_str(&prefix);
        text.push('\n');
    } else {
        text.push_str(
            "<runtime.notices></runtime.notices>\n<conversation.inbox></conversation.inbox>\n",
        );
    }
    text.push_str(&format!(
        "<input.message>{prompt}</input.message>\n<attachments></attachments>\n</kuku_turn_frame>"
    ));
    crate::context::CanonicalMessage::user_text(text)
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
            let request_id = format!(
                "req_{}",
                EventStore::replay(&pending.events_path)?.len() + 1
            );
            append_model_error(
                &pending.events_path,
                pending.turn,
                request_id,
                "missing_config",
                &error.to_string(),
            )?;
            append_turn_interrupted(
                &pending.events_path,
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
