use sha2::Digest;

use crate::context::{
    assemble_context, rebuild_history, restore_frozen_prelude, ContextInput, EnvironmentSource,
    FileSource, HistoryRange, RequestProvenance, SubagentRegistryProvenance,
    ToolRegistryProvenance,
};
use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::notice::types::{Notice, NoticeKind, NoticeSeverity};
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_body, NoticeAssemblyInput,
};
use crate::prompt::{builtin_handoff_instruction, load_prompt_template};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::tool;

use super::helpers::{
    append_model_error, append_turn_end, current_date_string, last_input_tokens,
    load_memory_sources, load_project_instruction_sources, now_timestamp, platform_label,
    provider_failure_kind, provider_format_name,
};
use super::tool_exec::record_plugin_hooks;
use super::types::{PendingRun, PendingStep, ResolvedRuntime, StreamingChunkState, UiEvent};

const MAX_REQUEST_LOOP: u64 = 20;

pub(super) async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
    ensure_resolved(&mut pending)?;
    pending.request_num += 1;
    check_loop_limit(&pending)?;

    let resolved = pending.resolved.as_ref().expect("resolved runtime exists");
    let existing_events = EventStore::replay(&pending.events_path)?;
    let (handoff_summary, history) = rebuild_history(&existing_events);
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

    let (runtime_blocks, notice_snapshots) = build_runtime_blocks(
        &pending.workspace,
        pending.turn,
        pending.subagent_registry.as_ref(),
        pending.skill_registry.as_mut(),
        &mut pending.skill_content_hash,
        &resolved.config,
        &pending.config.discovery,
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
            tools: tool::to_tool_schemas(&resolved.registry),
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
                Some(resolved.config.kind.as_str().to_string()),
                Some(resolved.config.model.clone()),
            )?;
            append_turn_end(&pending.events_path, pending.turn)?;
            return Err(error);
        }
    };

    let frozen = restore_frozen_prelude(&existing_events);
    let is_first_request = frozen.is_none();
    if let Some(frozen) = frozen {
        assembly.prelude_messages = frozen;
    }

    assembly.handoff_summary = handoff_summary;

    {
        let handoff_config = pending.config.handoff();
        if handoff_config.enabled {
            let estimated_input = last_input_tokens(&resolved.config.kind, &existing_events);
            let thinking_overhead = resolved.config.think_level.overhead_tokens();
            let headroom = compute_context_headroom(
                resolved
                    .config
                    .max_context_tokens
                    .saturating_sub(thinking_overhead),
                Some(resolved.config.max_output_tokens),
                estimated_input,
            );
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

    inject_runtime_context(
        &mut assembly.history,
        assembly.runtime_context.as_deref(),
        pending.skill_body.as_deref(),
    );

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
    let tier_name = pending
        .query
        .tier
        .clone()
        .unwrap_or_else(|| pending.config.default_tier().to_string());
    let think = resolved.config.think_level.as_str().to_string();
    let max_output = resolved.config.max_output_tokens;
    let params = serde_json::json!({
        "max_output_tokens": max_output,
        "temperature": pending.query.temperature,
    });

    let provenance =
        build_model_request_provenance(&pending, resolved, &assembly, &existing_events);

    {
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::ModelRequest {
            turn: pending.turn,
            ts: now_timestamp()?,
            request_id: request_id.clone(),
            tier: tier_name,
            think: think.clone(),
            provider: resolved.config.kind.as_str().to_string(),
            model: resolved.config.model.clone(),
            request_params: params,
            base_url: Some(resolved.config.base_url.clone()),
            history: Some(crate::event::types::RequestHistory {
                first: existing_events.first().map(|event| event.id),
                last: existing_events.last().map(|event| event.id),
                message_count: Some(1 + assembly.prelude_messages.len() + assembly.history.len()),
            }),
            tools: Some(crate::event::types::RequestTools {
                hash: Some(resolved.registry_hash.clone()),
                count: Some(resolved.tool_count),
                names: Some(resolved.tool_names.clone()),
            }),
            context: Some(crate::event::types::RequestContext {
                system: assembly.system_prompt.clone(),
                prelude: if is_first_request {
                    Some(prelude_snapshot)
                } else {
                    None
                },
                notices: notice_snapshots,
            }),
            provenance: Some(serde_json::to_value(&provenance)?),
        })?;
    }

    let request = crate::provider::types::ProviderRequest {
        assembly,
        catalog: &catalog,
        model: resolved.config.model.clone(),
        max_output_tokens: Some(max_output),
        temperature: pending.query.temperature,
        stream: true,
        think_level: think,
        thinking: resolved.config.thinking.clone(),
    };

    let mut lead_events = Vec::new();
    if pending.request_num == 1 {
        lead_events.push(UiEvent::TurnStart { turn: pending.turn });
    }
    lead_events.push(UiEvent::ModelRequest {
        model: resolved.config.model.clone(),
        provider: resolved.config.kind.as_str().to_string(),
    });

    let handoff_active = pending.handoff_triggered;
    match crate::provider::stream_provider(&resolved.config, &request).await {
        Ok(stream) => Ok(PendingStep::Streaming(Box::new(StreamingChunkState {
            pending,
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
        }))),
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
            store.append(EventPayload::HandoffTrigger {
                ts: now_timestamp()?,
                trigger: crate::event::HandoffTriggerReason::OverflowError,
            })?;
            store.append(EventPayload::Handoff {
                ts: now_timestamp()?,
                summary: user_input,
                kept_turns: pending.handoff_keep_turns,
            })?;
            store.append(EventPayload::TurnEnd {
                turn: pending.turn,
                ts: now_timestamp()?,
            })?;
            Err(crate::error::Error::Provider {
                kind: failure.kind,
                message: failure.message,
                provider: Some(resolved.config.kind.as_str().to_string()),
                model: Some(resolved.config.model.clone()),
            })
        }
        Err(failure) => {
            lead_events.push(UiEvent::Error {
                code: provider_failure_kind(&failure.kind).to_string(),
                message: failure.message.clone(),
            });
            append_model_error(
                &pending.events_path,
                pending.turn,
                request_id,
                provider_failure_kind(&failure.kind),
                &failure.message,
                Some(resolved.config.kind.as_str().to_string()),
                Some(resolved.config.model.clone()),
            )?;
            append_turn_end(&pending.events_path, pending.turn)?;
            Err(crate::error::Error::Provider {
                kind: failure.kind,
                message: failure.message,
                provider: Some(resolved.config.kind.as_str().to_string()),
                model: Some(resolved.config.model.clone()),
            })
        }
    }
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
            Some(provider_name.clone()),
            Some(model.clone()),
        )?;
        append_turn_end(&pending.events_path, pending.turn)?;
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
    turn: u64,
    subagent_registry: Option<&crate::subagent::registry::SubagentRegistry>,
    mut skill_registry: Option<&mut crate::skill::registry::SkillRegistry>,
    skill_content_hash: &mut Option<String>,
    resolved_config: &crate::provider::types::ResolvedProvider,
    discovery_config: &crate::config::DiscoveryConfig,
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

    if let Some(subagent_registry) = subagent_registry {
        if let Some(catalog_text) =
            crate::subagent::catalog::render_agent_catalog(subagent_registry)
        {
            parts.push(catalog_text);
        }
    }

    if let Some(ref mut skill_reg) = skill_registry {
        let new_registry = {
            crate::skill::registry::SkillRegistry::builder()
                .build_with_discovery(workspace, discovery_config)
                .map(|b| b.build())
                .ok()
        };

        if let Some(new_reg) = new_registry {
            let new_hash = new_reg.hash().to_string();
            if skill_content_hash.as_deref() != Some(&new_hash) {
                if let Some(changes) =
                    crate::skill::registry::detect_skill_changes(skill_reg, &new_reg)
                {
                    **skill_reg = new_reg;
                    *skill_content_hash = Some(new_hash);
                    let notice = Notice {
                        kind: NoticeKind::SkillChanged {
                            updated: changes.updated,
                            added: changes.added,
                            removed: changes.removed,
                        },
                        severity: NoticeSeverity::Info,
                    };
                    if let Some(body) = render_notice_body(&notice) {
                        notice_bodies.push(body);
                    }
                } else {
                    *skill_content_hash = Some(new_hash);
                }
            }
        }

        if let Some(catalog_text) = crate::skill::catalog::render_skill_catalog(skill_reg) {
            parts.push(catalog_text);
        }
    }

    if turn > 1 {
        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace,
            events: existing_events,
            context_budget_tier: context_headroom.tier,
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

fn inject_runtime_context(
    history: &mut [crate::context::CanonicalMessage],
    runtime_context: Option<&str>,
    skill_body: Option<&str>,
) {
    let Some(ctx) = runtime_context else { return };
    let Some(user_msg) = history.iter_mut().rev().find(|msg| {
        msg.role == crate::context::Role::User
            && msg
                .blocks
                .iter()
                .any(|b| matches!(b, crate::context::MessageBlock::Text(_)))
    }) else {
        return;
    };
    let mut new_blocks: Vec<crate::context::MessageBlock> = Vec::new();
    new_blocks.push(crate::context::MessageBlock::Text(ctx.to_string()));
    if let Some(sb) = skill_body {
        new_blocks.push(crate::context::MessageBlock::Text(sb.to_string()));
    }
    for block in &user_msg.blocks {
        new_blocks.push(block.clone());
    }
    user_msg.blocks = new_blocks;
}

fn build_model_request_provenance(
    pending: &PendingRun,
    resolved: &ResolvedRuntime,
    assembly: &crate::context::ContextAssembly,
    existing_events: &[crate::event::StoredEvent],
) -> crate::context::RequestProvenance {
    let headroom = {
        let estimated_input = last_input_tokens(&resolved.config.kind, existing_events);
        let thinking_overhead = resolved.config.think_level.overhead_tokens();
        compute_context_headroom(
            resolved
                .config
                .max_context_tokens
                .saturating_sub(thinking_overhead),
            Some(resolved.config.max_output_tokens),
            estimated_input,
        )
    };

    let request_id = format!("req_{}", pending.request_num);
    let tier_name = pending
        .query
        .tier
        .clone()
        .unwrap_or_else(|| pending.config.default_tier().to_string());
    let params = serde_json::json!({
        "max_output_tokens": resolved.config.max_output_tokens,
        "temperature": pending.query.temperature,
    });

    RequestProvenance {
        request_id,
        tier: tier_name,
        workspace: pending.workspace.display().to_string(),
        platform: platform_label().to_string(),
        current_date: current_date_string(),
        project_instruction_sources: assembly
            .project_instruction_sources
            .iter()
            .map(|s| FileSource {
                path: s.path.clone(),
                hash: s.hash.clone(),
            })
            .collect(),
        memory_sources: assembly
            .memory_sources
            .iter()
            .map(|s| FileSource {
                path: s.path.clone(),
                hash: s.hash.clone(),
            })
            .collect(),
        prompt_asset_sources: assembly.prompt_asset_sources.clone(),
        history_range: HistoryRange {
            first_event_id: existing_events.first().map(|e| e.id),
            last_event_id: existing_events.last().map(|e| e.id),
        },
        tool_registry: ToolRegistryProvenance {
            hash: resolved.registry_hash.clone(),
            names: resolved.tool_names.clone(),
            tool_count: resolved.tool_count,
        },
        subagent_registry: pending
            .subagent_registry
            .as_ref()
            .map(|r| SubagentRegistryProvenance {
                hash: r.hash().to_string(),
                names: r.names().to_vec(),
            }),
        skill_registry: pending.skill_registry.as_ref().map(|reg| {
            crate::context::provenance::SkillRegistryProvenance {
                hash: reg.hash().to_string(),
                names: reg.names().to_vec(),
            }
        }),
        plugin_registry: pending.plugin_registry.as_ref().map(|reg| {
            crate::context::provenance::PluginRegistryProvenance {
                hash: reg.hash().to_string(),
                names: reg.names().to_vec(),
                count: reg.len(),
            }
        }),
        provider_format: provider_format_name(&resolved.config.kind).to_string(),
        provider: resolved.config.kind.as_str().to_string(),
        model: resolved.config.model.clone(),
        request_params: params,
        token_estimate: None,
        context_budget_tier: headroom.tier.as_str().to_string(),
        max_context_tokens: Some(headroom.max_context_tokens),
        remaining_input_tokens: headroom.remaining_input_tokens,
    }
}

fn ensure_resolved(pending: &mut PendingRun) -> Result<()> {
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
                None,
                None,
            )?;
            append_turn_end(&pending.events_path, pending.turn)?;
            return Err(error);
        }
    };

    let registry = if let Some(ref overridden) = pending.tool_registry_override {
        overridden.clone()
    } else {
        tool::builtin_registry(!pending.query.disable_agents, !pending.query.disable_skills)
    };
    let registry_hash = tool::registry_hash(&registry);
    let tool_names = tool::tool_names(&registry);
    let tool_count = registry.len();
    if let Ok(policy_text) = std::fs::read_to_string(&pending.policy_path) {
        let policy_hash = sha2::Sha256::digest(policy_text.as_bytes());
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::PolicyLoaded {
            ts: now_timestamp()?,
            policy_hash: format!("sha256:{:x}", policy_hash),
            mode: "default".to_string(),
        })?;
    }

    pending.resolved = Some(ResolvedRuntime {
        config,
        registry,
        registry_hash,
        tool_names,
        tool_count,
    });
    Ok(())
}
