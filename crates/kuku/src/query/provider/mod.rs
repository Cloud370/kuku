use crate::context::{
    assemble_context, rebuild_history_for_provider, restore_prompt_snapshot,
    AgentRegistryProvenance, CanonicalMessage, ContextInput, EnvironmentSource, MessageBlock,
    PluginRegistryProvenance, PromptCapabilityMetadata, PromptRendererIdentity,
    RequestSnapshotBuilder, Role, SkillRegistryProvenance, SnapshotInput, ToolRegistryProvenance,
};
use crate::error::{Error, Result};
use crate::event::{
    CapabilityFact, CapabilityKind, CapabilityState, ContextBreakdown, ConversationContextFact,
    EventPayload, EventStore, InstructionContextFact, InstructionKind, ProviderFact, RequestCause,
    RequestScope, RequestStarted, RevisionToken, SourceFact, SourceScope, TaskActivityBatch,
    TaskEvent, TaskLedgerRecord, WorkspaceRelativePath,
};
use crate::log::{LogLevel, LogRecord, LogScope};
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_body, types::ContextHeadroom,
    NoticeAssemblyInput,
};
use crate::prompt::{builtin_handoff_instruction, load_prompt_template};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::tool;

use super::helpers::{
    append_model_error, append_turn_interrupted, current_date_string, last_input_tokens,
    load_memory_sources, load_project_instruction_sources, now_timestamp, platform_label,
};
use super::request::OwnedRequestBase;
use super::tool_exec::record_plugin_hooks;
use super::types::{PendingRun, PendingStep, ResolvedRuntime, StreamingChunkState, UiEvent};

mod request;

const MAX_REQUEST_LOOP: u64 = 20;

pub(super) async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
    ensure_resolved(&mut pending)?;
    pending.request_num += 1;
    check_loop_limit(&pending)?;

    let resolved = pending.resolved.as_ref().expect("resolved runtime exists");
    let resolved_config = resolved.config.clone();
    let registry = resolved.registry.clone();
    let existing_events = EventStore::replay(&pending.events_path)?;
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
            let request_id = format!("req_{}", pending.request_num);
            append_model_error(
                &pending.events_path,
                &pending.conversation,
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

    // Layer 4: inject agent catalog + loaded skills into snapshot prelude (first turn only;
    // subsequent turns reuse the frozen snapshot)
    if is_first_request {
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

    let prelude_snapshot = assembly.snapshot_prelude();
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
    let current_user_index = if let Some(index) = replace_current_user_message(
        &mut assembly.history,
        &pending.query.prompt,
        current_body,
        current_input.clone(),
    ) {
        index
    } else {
        assembly.history.push(current_input.clone());
        assembly.history.len() - 1
    };

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

    let current_input = assembly
        .history
        .get(current_user_index)
        .cloned()
        .ok_or_else(|| {
            crate::error::Error::InvalidEventStream(
                "assembled request has no current user message".to_string(),
            )
        })?;
    let catalog_revision = {
        use sha2::Digest;
        RevisionToken::parse(format!(
            "{:x}",
            sha2::Sha256::digest(catalog.hash().as_bytes())
        ))
        .expect("sha256 digest is a valid revision")
    };
    let request_base = OwnedRequestBase::new(
        assembly,
        catalog,
        crate::provider::types::CanonicalPromptInput {
            parts: vec![current_input],
        },
        resolved_config.model.clone(),
        max_output,
        pending.query.temperature,
        think,
        resolved_config.thinking.clone(),
        current_user_index,
    )?;
    pending.request_base = Some(request_base);

    if pending.execution_scope.is_some() {
        let base = pending
            .request_base
            .as_ref()
            .expect("request base captured");
        let execution = pending
            .execution_scope
            .as_ref()
            .expect("execution scope exists");
        let request_scope = RequestScope {
            execution: execution.clone(),
            request_id: crate::event::RequestId::derive_for_turn(
                execution,
                pending.turn,
                &request_id,
            )
            .map_err(|error| {
                Error::InvalidEventStream(format!("cannot derive task request id: {error}"))
            })?,
        };
        let cause = task_request_cause(&pending);
        let provider = provider_fact(&resolved_config.kind);
        let tier_id = pending.config.default_tier.clone();
        let snapshot = RequestSnapshotBuilder::build(SnapshotInput {
            scope: request_scope.clone(),
            cause: cause.clone(),
            provider,
            tier_id: &tier_id,
            assembly: base.snapshot_assembly(),
            handoff_context_template: base.snapshot_handoff_template(),
            allowlisted_provider_parameters: base.snapshot_parameters(),
            breakdown: request_breakdown_from_assembly(
                base.snapshot_assembly(),
                &existing_events,
                &request_scope,
                estimated_input,
            ),
            catalog_revision: catalog_revision.clone(),
        })
        .map_err(|error| Error::InvalidEventStream(format!("request snapshot failed: {error}")))?;
        let started = RequestStarted {
            scope: request_scope.clone(),
            cause,
            provider,
            model: resolved_config.model.clone(),
            started_at: now_timestamp()?,
        };
        append_task_request_activity(
            &pending.events_path,
            vec![
                TaskEvent::RequestSnapshot(snapshot),
                TaskEvent::RequestStarted(started.clone()),
            ],
        )?;
        pending.task_request = Some(super::types::TaskRequestLifecycle {
            scope: request_scope,
            started_at: std::time::Instant::now(),
        });
        pending.previous_task_request_id = Some(started.scope.request_id);
    }

    let provider_trace = Some(crate::provider::trace::ProviderTraceMetadata {
        kuku_home: pending.kuku_home.clone(),
        session_id: pending.session_id.clone(),
        turn: pending.turn,
        request_id: request_id.clone(),
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
            "request_id": request_id,
        })),
    )?;
    lead_events.extend(pending.pending_events.drain(..));
    lead_events.push(UiEvent::ModelRequest {
        model: model_name,
        provider: provider_name,
        conversation: pending.conversation.clone(),
        turn: pending.turn,
        request_id: request_id.clone(),
        request_ordinal: pending.model_request_count + 1,
    });
    if pending.request_num == 1 {
        lead_events.push(UiEvent::TurnStart { turn: pending.turn });
    }

    let handoff_active = pending.handoff_triggered;
    let request = pending
        .request_base
        .as_ref()
        .expect("request base captured")
        .request();
    match crate::provider::stream_provider(&resolved_config, &request, provider_trace).await {
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
                tool_call_completions: Vec::new(),
                tool_stream_invalid: false,
                terminal_stream_invalid: false,
                stream_ended: false,
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
            let mut store = EventStore::open(&pending.events_path)?;
            store.append(EventPayload::Handoff {
                turn: pending.turn,
                ts: now_timestamp()?,
                request_id: request_id.clone(),
                summary: user_input,
                keep_turns: pending.handoff_keep_turns,
            })?;
            store.append(EventPayload::ModelError {
                conversation: Some(pending.conversation.as_str().to_string()),
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
            record_task_request_failed(
                &mut pending,
                failure.provider_request_id.clone(),
                None,
                failure.kind,
                &failure.message,
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
                &pending.conversation,
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
            record_task_request_failed(
                &mut pending,
                failure.provider_request_id.clone(),
                None,
                failure.kind,
                &failure.message,
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

pub(super) fn pending_failure_step(
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
            &pending.conversation,
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

fn should_trigger_handoff(headroom: &ContextHeadroom, threshold: f64) -> bool {
    let Some(remaining) = headroom.remaining_input_tokens else {
        return false;
    };
    let budget = headroom
        .max_context_tokens
        .saturating_sub(headroom.reserved_output_tokens)
        .saturating_sub(headroom.reserved_margin_tokens);
    if budget == 0 {
        return false;
    }

    let used_ratio = 1.0 - (f64::from(remaining) / f64::from(budget));
    used_ratio >= threshold
}

#[allow(clippy::too_many_arguments)]
pub(super) fn request_breakdown_from_assembly(
    assembly: &crate::context::ContextAssembly,
    events: &[crate::event::StoredEvent],
    scope: &RequestScope,
    estimated_input: Option<u32>,
) -> ContextBreakdown {
    let mut skills = Vec::<crate::event::SkillContextFact>::new();
    let mut observations = Vec::new();
    for stored in events {
        let EventPayload::TaskLedger(record) = &stored.payload else {
            continue;
        };
        let record_events = match record {
            TaskLedgerRecord::Control(transaction) => transaction.events(),
            TaskLedgerRecord::Activity(batch) => batch.events(),
        };
        for event in record_events {
            match event {
                TaskEvent::SkillLoaded(skill)
                    if skill.execution == scope.execution
                        && !skills.iter().any(|fact| fact.skill_id == skill.skill_id) =>
                {
                    skills.push(crate::event::SkillContextFact {
                        skill_id: skill.skill_id.clone(),
                        source: skill.source.clone(),
                        origin: skill.origin,
                        content_hash: skill.content_hash.clone(),
                    });
                }
                TaskEvent::ObservationRecorded(observation) if observation.scope == *scope => {
                    observations.push(observation.clone());
                }
                _ => {}
            }
        }
    }
    let instructions =
        assembly
            .prompt_asset_sources
            .iter()
            .map(|source| InstructionContextFact {
                kind: InstructionKind::System,
                source: source_fact(SourceScope::System, "prompt", source),
                content_hash: source.hash.clone(),
            })
            .chain(assembly.project_instruction_sources.iter().map(|source| {
                InstructionContextFact {
                    kind: match source.kind.as_str() {
                        "workspace" => InstructionKind::Workspace,
                        "agent" => InstructionKind::Agent,
                        _ => InstructionKind::Project,
                    },
                    source: source_fact(
                        SourceScope::Project,
                        "instruction",
                        &crate::context::provenance::FileSource {
                            path: source.path.clone(),
                            hash: source.hash.clone(),
                        },
                    ),
                    content_hash: source.hash.clone(),
                }
            }))
            .collect();
    let memory = assembly
        .memory_sources
        .iter()
        .map(|source| crate::event::MemoryContextFact {
            kind: if source.path.contains("global") {
                crate::event::MemoryKind::Global
            } else {
                crate::event::MemoryKind::Project
            },
            source: source_fact(
                if source.path.contains("global") {
                    SourceScope::User
                } else {
                    SourceScope::Project
                },
                "memory",
                &crate::context::provenance::FileSource {
                    path: source.path.clone(),
                    hash: source.hash.clone(),
                },
            ),
            content_hash: source.hash.clone(),
        })
        .collect();
    let tool_names = assembly
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let capabilities = [
        (
            CapabilityKind::FileRead,
            ["read_file", "find_files", "search_text"]
                .iter()
                .any(|name| tool_names.contains(name)),
        ),
        (
            CapabilityKind::FileWrite,
            ["write_file", "edit_file"]
                .iter()
                .any(|name| tool_names.contains(name)),
        ),
        (
            CapabilityKind::CommandExecution,
            tool_names.contains("run_command"),
        ),
        (
            CapabilityKind::NetworkAccess,
            tool_names.contains("fetch_web"),
        ),
        (
            CapabilityKind::AgentDelegation,
            tool_names.contains("agent"),
        ),
        (
            CapabilityKind::SkillDiscovery,
            ["use_skill", "search_skills"]
                .iter()
                .any(|name| tool_names.contains(name)),
        ),
        (CapabilityKind::Memory, !assembly.memory_sources.is_empty()),
    ]
    .into_iter()
    .filter(|(_, available)| *available)
    .map(|(kind, _)| CapabilityFact {
        kind,
        state: CapabilityState::Available,
    })
    .collect();
    ContextBreakdown {
        skills,
        instructions,
        memory,
        conversation: ConversationContextFact {
            retained_turns: assembly
                .history
                .iter()
                .filter(|message| message.role == Role::User)
                .count() as u64,
            handoff_boundaries: u64::from(assembly.handoff_summary.is_some()),
            history_summarized: assembly.handoff_summary.is_some(),
            delegated_results: Vec::new(),
        },
        observations,
        delegated_results: Vec::new(),
        capabilities,
        token_estimate: estimated_input.map(u64::from),
    }
}

fn source_fact(
    scope: SourceScope,
    prefix: &str,
    source: &crate::context::provenance::FileSource,
) -> SourceFact {
    SourceFact {
        scope,
        id: format!("{prefix}:{}", source.hash),
        relative_path: WorkspaceRelativePath::parse(&source.path).ok(),
    }
}

fn task_request_cause(pending: &PendingRun) -> RequestCause {
    pending
        .previous_task_request_id
        .as_ref()
        .map(|parent_request_id| RequestCause::ToolContinuation {
            parent_request_id: parent_request_id.clone(),
        })
        .unwrap_or(RequestCause::UserSubmission)
}

pub(super) fn append_task_request_activity(
    events_path: &std::path::Path,
    events: Vec<TaskEvent>,
) -> Result<()> {
    let batch = TaskActivityBatch::try_new(events).map_err(|error| {
        Error::InvalidEventStream(format!("invalid task request lifecycle activity: {error}"))
    })?;
    EventStore::open(events_path)?
        .append(EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)))?;
    Ok(())
}

pub(super) fn record_task_request_completed(
    pending: &mut PendingRun,
    provider_request_id: Option<String>,
    usage: Option<&crate::provider::types::ProviderUsage>,
) -> Result<()> {
    let Some(lifecycle) = pending.task_request.take() else {
        return Ok(());
    };
    append_task_request_activity(
        &pending.events_path,
        vec![TaskEvent::RequestCompleted(self::request::completed(
            lifecycle.scope,
            lifecycle.started_at,
            provider_request_id,
            usage,
        ))],
    )
}

pub(super) fn record_task_request_failed(
    pending: &mut PendingRun,
    provider_request_id: Option<String>,
    usage: Option<&crate::provider::types::ProviderUsage>,
    kind: crate::provider::types::ProviderFailureKind,
    summary: &str,
) -> Result<()> {
    let Some(lifecycle) = pending.task_request.take() else {
        return Ok(());
    };
    append_task_request_activity(
        &pending.events_path,
        vec![TaskEvent::RequestFailed(self::request::failed(
            lifecycle.scope,
            lifecycle.started_at,
            provider_request_id,
            usage,
            kind,
            summary.to_string(),
        ))],
    )
}

pub(super) fn provider_fact(kind: &crate::provider::types::ProviderKind) -> ProviderFact {
    match kind {
        crate::provider::types::ProviderKind::Anthropic => ProviderFact::Anthropic,
        crate::provider::types::ProviderKind::OpenAiCompatible => ProviderFact::OpenAiCompatible,
        crate::provider::types::ProviderKind::OpenAiResponses => ProviderFact::OpenAiResponses,
    }
}

fn build_runtime_blocks(
    workspace: &std::path::Path,
    conversation: &str,
    turn: u64,
    agent_registry: Option<&crate::agent::registry::AgentRegistry>,
    skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    previous_skill_registry: Option<&crate::skill::registry::SkillRegistry>,
    resolved_config: &crate::provider::types::ResolvedProvider,
    existing_events: &[crate::event::StoredEvent],
    catalog: &crate::prompt::PromptCatalog,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let estimated_input = last_input_tokens(&resolved_config.kind, existing_events);
    let thinking_overhead = resolved_config.think_level.overhead_tokens();
    let context_headroom = compute_context_headroom(
        resolved_config
            .max_context_tokens
            .saturating_sub(thinking_overhead),
        Some(resolved_config.max_output_tokens),
        estimated_input,
    );

    // Build agent catalog — this goes into snapshot prelude, not runtime_blocks
    let catalog_text =
        agent_registry.and_then(|reg| crate::agent::catalog::render_agent_catalog(reg, catalog));

    // Build skill catalog — this goes into snapshot prelude, not runtime_blocks
    let skills_text = skill_registry.and_then(|skill_reg| {
        let loaded_skill_names =
            crate::skill::session::loaded_skill_names(existing_events, conversation);
        let skill_changes = if turn > 1 {
            previous_skill_registry.and_then(|previous_skill_registry| {
                crate::skill::registry::detect_skill_changes(previous_skill_registry, skill_reg)
            })
        } else {
            None
        };
        crate::skill::catalog::render_skill_catalog(
            skill_reg,
            &loaded_skill_names,
            skill_changes.as_ref(),
        )
    });

    // Dynamic notices remain in runtime_blocks (conversations, inbox, drift)
    let mut notice_bodies: Vec<String> = Vec::new();

    if turn > 1 {
        let conversation = crate::conversation::address::ConversationAddress::parse(conversation)
            .unwrap_or(crate::conversation::address::ConversationAddress::MAIN);
        let notice_events = existing_events
            .iter()
            .filter(|event| {
                !matches!(
                    &event.payload,
                    crate::event::EventPayload::MessageUser {
                        conversation: event_conversation,
                        from: Some(_),
                        via_tool_call_id: Some(_),
                        ..
                    } if event_conversation == conversation.as_str()
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace,
            events: &notice_events,
            context_budget_tier: context_headroom.tier,
            conversation: &conversation,
            agent_registry,
        });
        for notice in &notices {
            if let Some(body) = render_notice_body(notice, catalog) {
                notice_bodies.push(body);
            }
        }
    }

    // Notices are now self-wrapped with <kuku_system_notice> via templates;
    // just join them without adding an outer wrapper.
    let runtime_blocks = if notice_bodies.is_empty() {
        None
    } else {
        Some(notice_bodies.join("\n\n"))
    };

    Ok((catalog_text, skills_text, runtime_blocks))
}

fn assembly_runtime_prefix(
    runtime_context: Option<&str>,
    skill_body: Option<&str>,
    catalog: &crate::prompt::PromptCatalog,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(runtime_context) = runtime_context.filter(|value| !value.is_empty()) {
        let tmpl = catalog
            .blocks
            .get("runtime-notices")
            .map(|a| a.text.as_str())
            .unwrap_or("<kuku_runtime_notices>{{runtime_notices_content}}</kuku_runtime_notices>");
        parts.push(tmpl.replace("{{runtime_notices_content}}", runtime_context));
    }
    if let Some(skill_body) = skill_body.filter(|value| !value.is_empty()) {
        let tmpl = catalog
            .blocks
            .get("conversation-inbox")
            .map(|a| a.text.as_str())
            .unwrap_or(
                "<kuku_conversation_inbox>{{conversation_inbox_content}}</kuku_conversation_inbox>",
            );
        parts.push(tmpl.replace("{{conversation_inbox_content}}", skill_body));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn append_handoff_instruction(
    prefix: Option<String>,
    instruction: &str,
    catalog: &crate::prompt::PromptCatalog,
) -> Option<String> {
    let Some(prefix) = prefix else {
        let notices_tmpl = catalog
            .blocks
            .get("runtime-notices")
            .map(|a| a.text.as_str())
            .unwrap_or("<kuku_runtime_notices>{{runtime_notices_content}}</kuku_runtime_notices>");
        let inbox_tmpl = catalog
            .blocks
            .get("conversation-inbox")
            .map(|a| a.text.as_str())
            .unwrap_or(
                "<kuku_conversation_inbox>{{conversation_inbox_content}}</kuku_conversation_inbox>",
            );
        return Some(format!(
            "{}\n{}",
            notices_tmpl.replace("{{runtime_notices_content}}", instruction),
            inbox_tmpl.replace("{{conversation_inbox_content}}", ""),
        ));
    };

    if let Some(index) = prefix.rfind("</kuku_runtime_notices>") {
        let (before, after) = prefix.split_at(index);
        Some(format!("{before}\n\n{instruction}{after}"))
    } else {
        Some(format!("{prefix}\n{instruction}"))
    }
}

fn build_current_user_message(
    prefix: Option<String>,
    prompt: &str,
) -> crate::context::CanonicalMessage {
    let mut blocks = Vec::new();
    if let Some(prefix) = prefix {
        blocks.push(crate::context::MessageBlock::Text(prefix));
    }
    blocks.push(crate::context::MessageBlock::Text(prompt.to_string()));
    crate::context::CanonicalMessage::user(blocks)
}

fn replace_latest_user_message(
    history: &mut [CanonicalMessage],
    prompt: &str,
    replacement: CanonicalMessage,
) -> Option<usize> {
    for (index, message) in history.iter_mut().enumerate().rev() {
        if message.role != Role::User || message.blocks.len() != 1 {
            continue;
        }
        let MessageBlock::Text(text) = &message.blocks[0] else {
            continue;
        };
        if text == prompt {
            *message = replacement;
            return Some(index);
        }
    }
    None
}

fn replace_current_user_message(
    history: &mut [CanonicalMessage],
    raw_prompt: &str,
    current_body: &str,
    replacement: CanonicalMessage,
) -> Option<usize> {
    if current_body != raw_prompt {
        if let Some(index) = replace_latest_user_message(history, current_body, replacement.clone())
        {
            return Some(index);
        }
    }
    replace_latest_user_message(history, raw_prompt, replacement)
}

fn append_current_turn_prefix_once(messages: &mut Vec<CanonicalMessage>, prefix: &str) {
    if messages.iter().any(|message| {
        message.blocks.iter().any(|block| match block {
            MessageBlock::Text(text) => text.contains(prefix),
            MessageBlock::Thinking(_) | MessageBlock::ToolUse(_) | MessageBlock::ToolResult(_) => {
                false
            }
        })
    }) {
        return;
    }
    messages.push(CanonicalMessage::user_text(prefix.to_string()));
}

fn insert_current_turn_metadata_block(message: &mut CanonicalMessage, text: String) {
    let insert_at = message.blocks.len().saturating_sub(1);
    message
        .blocks
        .insert(insert_at, crate::context::MessageBlock::Text(text));
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
                &pending.conversation,
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

#[cfg(test)]
mod tests;
