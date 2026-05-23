use sha2::Digest;

use crate::context::{
    assemble_context, build_request_provenance, rebuild_history, ContextInput, EnvironmentSource,
    FileSource, HistoryRange, RequestProvenanceInput, SubagentRegistryProvenance,
    ToolRegistryProvenance,
};
use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::notice::types::{Notice, NoticeKind, NoticeSeverity};
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_body, NoticeAssemblyInput,
};
use crate::permission::{
    decide_tool_call, load_project_policy, recover_session_grants, GateDecisionKind, GateSource,
};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::tool;

use super::helpers::{
    append_model_error, append_permission_decision, append_permission_request, append_turn_end,
    current_date_string, display_summary, execute_tool_call, gate_source_name, last_input_tokens,
    load_memory_sources, load_project_instruction_sources, now_timestamp, permission_candidate,
    permission_rule, platform_label, provider_failure_kind, provider_format_name,
};
use super::run::find_tool_definition;
use super::types::{
    PendingPermission, PendingRun, PendingStep, PermissionChoice, PermissionRequest,
    QueuedToolCall, ResolvedRuntime, StreamingChunkState, UiEvent,
};

const MAX_REQUEST_LOOP: u64 = 20;

pub(super) async fn finish_streaming(state: StreamingChunkState) -> Result<PendingStep> {
    let StreamingChunkState {
        mut pending,
        request_id,
        accumulated_text,
        accumulated_thinking,
        stop_reason,
        tool_calls,
        usage,
        ..
    } = state;

    if let Some(ref u) = usage {
        pending.cumulative_input_tokens += u.input_tokens.unwrap_or(0);
        pending.cumulative_output_tokens += u.output_tokens.unwrap_or(0);
        pending.cumulative_cache_read_input_tokens += u.cache_read_input_tokens.unwrap_or(0);
        pending.cumulative_cache_creation_input_tokens +=
            u.cache_creation_input_tokens.unwrap_or(0);
    }

    let has_tool_calls = !tool_calls.is_empty();
    let final_stop_reason = stop_reason.unwrap_or_else(|| {
        if has_tool_calls {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        }
    });

    {
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::ModelResponse {
            turn: pending.turn,
            ts: now_timestamp()?,
            request_id: request_id.clone(),
            text: accumulated_text.clone(),
            thinking: if accumulated_thinking.is_empty() {
                None
            } else {
                Some(accumulated_thinking.clone())
            },
            stop_reason: final_stop_reason.clone(),
            tool_call_count: has_tool_calls.then_some(tool_calls.len() as u64),
            usage: serde_json::to_value(&usage).unwrap_or_default(),
        })?;

        if !has_tool_calls {
            store.append(EventPayload::TurnEnd {
                turn: pending.turn,
                ts: now_timestamp()?,
            })?;
            let total_usage = Some(crate::provider::types::ProviderUsage {
                input_tokens: Some(pending.cumulative_input_tokens),
                output_tokens: Some(pending.cumulative_output_tokens),
                cache_read_input_tokens: Some(pending.cumulative_cache_read_input_tokens),
                cache_creation_input_tokens: Some(pending.cumulative_cache_creation_input_tokens),
            });
            return Ok(PendingStep::Done(
                super::types::RunOutput {
                    session_id: pending.session_id.clone(),
                    text: accumulated_text,
                    usage: total_usage.clone(),
                    turn: pending.turn,
                },
                total_usage,
                pending.turn,
            ));
        }

        for tool_call in &tool_calls {
            store.append(EventPayload::ToolCall {
                turn: pending.turn,
                ts: now_timestamp()?,
                tool_call_id: tool_call.id.clone(),
                request_id: request_id.clone(),
                index: tool_call.index,
                tool: tool_call.name.clone(),
                args: tool_call.args.clone(),
            })?;
        }
    }

    for tool_call in tool_calls {
        let display = display_summary(&tool_call.name, &tool_call.args, None);
        pending.queued_tool_calls.push_back(QueuedToolCall {
            tool_call,
            display_summary: display,
        });
    }

    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: None,
    })
}

pub(super) async fn advance_pending(mut pending: PendingRun) -> Result<PendingStep> {
    {
        {
            let notified = pending.cancel_token.notified();
            tokio::pin!(notified);
            if notified.enable() {
                let mut store = EventStore::open(&pending.events_path)?;
                store.append(EventPayload::TurnEnd {
                    turn: pending.turn,
                    ts: now_timestamp()?,
                })?;
                return Ok(PendingStep::Done(
                    super::types::RunOutput {
                        session_id: pending.session_id.clone(),
                        text: String::new(),
                        usage: None,
                        turn: pending.turn,
                    },
                    None,
                    pending.turn,
                ));
            }
        }

        if !pending.queued_tool_calls.is_empty() {
            let all_calls: Vec<_> = pending.queued_tool_calls.drain(..).collect();
            let (agent_calls, rest_calls): (Vec<_>, Vec<_>) = all_calls
                .into_iter()
                .partition(|c| c.tool_call.name == "agent");
            let (skill_calls, regular_calls): (Vec<_>, Vec<_>) = rest_calls
                .into_iter()
                .partition(|c| c.tool_call.name == "use_skill");

            for call in agent_calls {
                let step = Box::pin(handle_agent_tool_call(pending, call)).await?;
                match step {
                    PendingStep::Pending(p) => pending = *p,
                    other => return Ok(other),
                }
            }

            let policy = load_project_policy(&pending.policy_path)?;
            let prior_events = EventStore::replay(&pending.events_path)?;
            let session_grants = recover_session_grants(&prior_events);

            let mut dispatch_batch = Vec::new();
            let mut ui_events = Vec::new();

            for call in skill_calls {
                ui_events.push(UiEvent::ToolStart {
                    id: call.tool_call.id.clone(),
                    tool: call.tool_call.name.clone(),
                    summary: call.display_summary.clone(),
                    kind: super::types::ToolKind::Simple,
                });
                let result = execute_tool_call(&mut pending, &call.tool_call).await?;
                ui_events.push(UiEvent::ToolEnd {
                    id: call.tool_call.id.clone(),
                    status: result.status,
                    summary: result.summary,
                    result: result.structured,
                });
            }

            for queued in regular_calls {
                let definition = find_tool_definition(&pending, &queued.tool_call.name)
                    .ok_or_else(|| {
                        crate::error::Error::InvalidArgument(format!(
                            "unknown tool: {}",
                            queued.tool_call.name
                        ))
                    })?;
                let candidate = permission_candidate(
                    &pending.kuku_home,
                    &pending.workspace,
                    &queued.tool_call.name,
                    &queued.tool_call.args,
                );
                let decision = decide_tool_call(
                    &queued.tool_call.name,
                    &definition.risk,
                    &candidate,
                    &policy,
                    &session_grants,
                );
                match decision.kind {
                    GateDecisionKind::Ask => {
                        let request = PermissionRequest {
                            id: queued.tool_call.id.clone(),
                            tool_call_id: queued.tool_call.id.clone(),
                            tool: queued.tool_call.name.clone(),
                            risk: definition.risk.clone(),
                            summary: queued.display_summary.clone(),
                        };
                        append_permission_request(&pending.events_path, pending.turn, &request)?;
                        pending.queued_tool_calls.push_front(queued);
                        return Ok(PendingStep::NeedPermission(Box::new(PendingPermission {
                            pending,
                            request,
                        })));
                    }
                    GateDecisionKind::Allow => {
                        if !matches!(decision.source, GateSource::TrustPosture) {
                            let choice = if matches!(decision.source, GateSource::ProjectPolicy) {
                                PermissionChoice::Project
                            } else if matches!(decision.source, GateSource::SessionGrant) {
                                PermissionChoice::Session
                            } else {
                                PermissionChoice::Once
                            };
                            append_permission_decision(
                                &pending.events_path,
                                pending.turn,
                                &queued.tool_call.id,
                                choice,
                                gate_source_name(decision.source),
                                &permission_rule(
                                    &pending.kuku_home,
                                    &pending.workspace,
                                    &queued.tool_call.name,
                                    &queued.tool_call.args,
                                ),
                            )?;
                        }
                        ui_events.push(UiEvent::ToolStart {
                            id: queued.tool_call.id.clone(),
                            tool: queued.tool_call.name.clone(),
                            summary: queued.display_summary.clone(),
                            kind: super::types::ToolKind::Simple,
                        });
                        let result_event_id = EventStore::open(&pending.events_path)?.next_id()
                            + dispatch_batch.len() as u64;
                        dispatch_batch.push((
                            queued.tool_call.index as usize,
                            queued.tool_call.id.clone(),
                            queued.tool_call.name.clone(),
                            queued.tool_call.args.clone(),
                            pending.workspace.clone(),
                            pending.kuku_home.clone(),
                            prior_events.clone(),
                            result_event_id,
                        ));
                    }
                    GateDecisionKind::Deny => {
                        append_permission_request(
                            &pending.events_path,
                            pending.turn,
                            &PermissionRequest {
                                id: queued.tool_call.id.clone(),
                                tool_call_id: queued.tool_call.id.clone(),
                                tool: queued.tool_call.name.clone(),
                                risk: definition.risk.clone(),
                                summary: queued.display_summary.clone(),
                            },
                        )?;
                        append_permission_decision(
                            &pending.events_path,
                            pending.turn,
                            &queued.tool_call.id,
                            PermissionChoice::Deny,
                            gate_source_name(decision.source),
                            &permission_rule(
                                &pending.kuku_home,
                                &pending.workspace,
                                &queued.tool_call.name,
                                &queued.tool_call.args,
                            ),
                        )?;
                        let result = execute_tool_call(&mut pending, &queued.tool_call).await?;
                        ui_events.push(UiEvent::ToolEnd {
                            id: queued.tool_call.id.clone(),
                            status: result.status,
                            summary: result.summary,
                            result: result.structured,
                        });
                    }
                }
            }

            for (_index, tc_id, name, args, workspace, kuku_home, prior_events, result_event_id) in
                dispatch_batch
            {
                let result = crate::tool::dispatch::dispatch(
                    &name,
                    &args,
                    &workspace,
                    &kuku_home,
                    &prior_events,
                    result_event_id,
                    None,
                )
                .await;
                let mut store = EventStore::open(&pending.events_path)?;
                store.append(crate::event::EventPayload::ToolResult {
                    turn: pending.turn,
                    ts: now_timestamp()?,
                    tool_call_id: tc_id,
                    status: result.status.clone(),
                    summary: result.summary.clone(),
                    model_content: result.model_content.clone(),
                    truncated: result.truncated,
                    structured: result.structured.clone(),
                })?;
            }

            if !ui_events.is_empty() {
                return Ok(PendingStep::BatchReady {
                    pending: Box::new(pending),
                    ui_events,
                });
            }
            return Ok(PendingStep::Pending {
                pending: Box::new(pending),
                slot: None,
                event: None,
            });
        }

        call_provider_step(pending).await
    }
}

async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
    ensure_resolved(&mut pending)?;
    pending.request_num += 1;

    if pending.request_num > MAX_REQUEST_LOOP {
        let provider_name = pending
            .resolved
            .as_ref()
            .map(|resolved| resolved.config.kind.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let model = pending
            .resolved
            .as_ref()
            .map(|resolved| resolved.config.model.clone())
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

    let resolved = pending.resolved.as_ref().expect("resolved runtime exists");
    let existing_events = EventStore::replay(&pending.events_path)?;
    let history = rebuild_history(&existing_events);
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

    // Compute context headroom for notice budget
    let estimated_input = last_input_tokens(&resolved.config.kind, &existing_events);
    let thinking_overhead: u32 = match resolved.config.think_level {
        crate::config::ThinkLevel::Off => 0,
        crate::config::ThinkLevel::Low => 1024,
        crate::config::ThinkLevel::Medium => 4096,
        crate::config::ThinkLevel::High => 16000,
    };
    let context_headroom = compute_context_headroom(
        resolved
            .config
            .max_context_tokens
            .saturating_sub(thinking_overhead),
        Some(resolved.config.max_output_tokens),
        estimated_input,
    );

    // Build runtime_blocks: agent catalog + notices
    let mut runtime_blocks_parts: Vec<String> = Vec::new();
    let mut notice_bodies: Vec<String> = Vec::new();
    let mut notice_snapshots: Vec<crate::event::types::ContextMessage> = Vec::new();

    // Agent catalog
    if let Some(ref subagent_registry) = pending.subagent_registry {
        if let Some(catalog_text) =
            crate::subagent::catalog::render_agent_catalog(subagent_registry)
        {
            runtime_blocks_parts.push(catalog_text);
        }
    }

    // Skill catalog (with hot-reload)
    if let Some(ref skill_registry) = pending.skill_registry {
        let new_registry = {
            let builder = crate::skill::registry::SkillRegistry::builder()
                .load_claude_user_skills()
                .and_then(|b| b.load_claude_project_skills(&pending.workspace))
                .and_then(|b| b.load_opencode_user_skills())
                .and_then(|b| b.load_opencode_project_skills(&pending.workspace))
                .and_then(|b| b.load_kuku_user_skills())
                .and_then(|b| b.load_kuku_project_skills(&pending.workspace));
            builder.map(|b| b.build()).ok()
        };

        if let Some(new_reg) = new_registry {
            let new_hash = new_reg.hash().to_string();
            if pending.skill_content_hash.as_deref() != Some(&new_hash) {
                if let Some(changes) =
                    crate::skill::registry::detect_skill_changes(skill_registry, &new_reg)
                {
                    pending.skill_registry = Some(new_reg);
                    pending.skill_content_hash = Some(new_hash);
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
                    pending.skill_content_hash = Some(new_hash);
                }
            }
        }

        if let Some(catalog_text) =
            crate::skill::catalog::render_skill_catalog(pending.skill_registry.as_ref().unwrap())
        {
            runtime_blocks_parts.push(catalog_text);
        }
    }

    // Context drift notices (turn > 1)
    if pending.turn > 1 {
        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: &pending.workspace,
            events: &existing_events,
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

    // Merge all notice bodies into a single <kuku_system_notice>
    if !notice_bodies.is_empty() {
        let merged = notice_bodies.join("\n\n");
        runtime_blocks_parts.push(format!(
            "<kuku_system_notice>\n{merged}\n</kuku_system_notice>"
        ));
    }

    let runtime_blocks = if runtime_blocks_parts.is_empty() {
        None
    } else {
        Some(runtime_blocks_parts.join("\n"))
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
        catalog,
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
    let prelude_snapshot = assembly.snapshot_prelude();

    // Inject runtime_context and skill_body into the last user message that has text blocks
    if let Some(ref runtime_context) = assembly.runtime_context {
        if let Some(user_msg) = assembly.history.iter_mut().rev().find(|msg| {
            msg.role == crate::context::Role::User
                && msg
                    .blocks
                    .iter()
                    .any(|b| matches!(b, crate::context::MessageBlock::Text(_)))
        }) {
            let mut new_blocks: Vec<crate::context::MessageBlock> = Vec::new();
            new_blocks.push(crate::context::MessageBlock::Text(runtime_context.clone()));
            if let Some(ref sb) = pending.skill_body {
                new_blocks.push(crate::context::MessageBlock::Text(sb.clone()));
            }
            // Preserve all existing blocks (text + tool results)
            for block in &user_msg.blocks {
                new_blocks.push(block.clone());
            }
            user_msg.blocks = new_blocks;
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

    let provenance = build_request_provenance(RequestProvenanceInput {
        request_id: request_id.clone(),
        tier: tier_name.clone(),
        workspace: pending.workspace.display().to_string(),
        platform: platform.clone(),
        current_date: current_date.clone(),
        project_instruction_sources: assembly
            .project_instruction_sources
            .iter()
            .map(|source| FileSource {
                path: source.path.clone(),
                hash: source.hash.clone(),
            })
            .collect(),
        memory_sources: assembly
            .memory_sources
            .iter()
            .map(|source| FileSource {
                path: source.path.clone(),
                hash: source.hash.clone(),
            })
            .collect(),
        prompt_asset_sources: assembly.prompt_asset_sources.clone(),
        history_range: HistoryRange {
            first_event_id: existing_events.first().map(|event| event.id),
            last_event_id: existing_events.last().map(|event| event.id),
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
        provider_format: provider_format_name(&resolved.config.kind).to_string(),
        provider: resolved.config.kind.as_str().to_string(),
        model: resolved.config.model.clone(),
        request_params: params.clone(),
        token_estimate: None,
        context_budget_tier: context_headroom.tier.as_str().to_string(),
        max_context_tokens: Some(context_headroom.max_context_tokens),
        remaining_input_tokens: context_headroom.remaining_input_tokens,
    });

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
                prelude: prelude_snapshot,
                notices: notice_snapshots,
            }),
            provenance: Some(serde_json::to_value(&provenance)?),
        })?;
    }

    let request = crate::provider::types::ProviderRequest {
        assembly,
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
        }))),
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

async fn handle_agent_tool_call(
    pending: PendingRun,
    queued_tool_call: QueuedToolCall,
) -> Result<PendingStep> {
    let name = queued_tool_call
        .tool_call
        .args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let prompt = queued_tool_call
        .tool_call
        .args
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let definition = pending
        .subagent_registry
        .as_ref()
        .and_then(|reg| reg.get(&name))
        .cloned()
        .ok_or_else(|| crate::error::Error::Provider {
            kind: crate::provider::types::ProviderFailureKind::Unknown,
            message: format!("subagent '{}' not found in registry", &name),
            provider: None,
            model: None,
        })?;

    if pending.child_session_count >= 2 {
        let mut store = EventStore::open(&pending.events_path)?;
        store.append(EventPayload::ToolResult {
            turn: pending.turn,
            ts: now_timestamp()?,
            tool_call_id: queued_tool_call.tool_call.id.clone(),
            status: "blocked".to_string(),
            summary: "blocked: maximum subagent sessions (20) reached".to_string(),
            model_content: String::new(),
            truncated: false,
            structured: None,
        })?;
        return Ok(PendingStep::BatchReady {
            pending: Box::new(pending),
            ui_events: vec![UiEvent::ToolEnd {
                id: queued_tool_call.tool_call.id.clone(),
                status: "blocked".to_string(),
                summary: "blocked: maximum subagent sessions (20) reached".to_string(),
                result: None,
            }],
        });
    }

    let child_session_id = format!(
        "child_{}_{}",
        pending.session_id, pending.child_session_count
    );
    let _stage_id = child_session_id.clone();
    let _label = format!("{} · {}", &name, truncate_summary(&prompt, 60));
    let _tool_call_id = queued_tool_call.tool_call.id.clone();

    let mut pending = pending;
    pending.child_session_count += 1;

    // TODO: Task 3/4 — replace with spawn_agent_slot
    let _child_run = crate::subagent::session::start_child_session(
        pending.events_path.parent().unwrap(),
        &child_session_id,
        &definition,
        &prompt,
        &pending.workspace,
        &pending.kuku_home,
        pending.config.clone(),
        pending.prompts_dir.as_deref(),
        super::types::PermissionMode::AutoAllow,
        pending.child_session_count,
    )
    .await?;

    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: None,
    })
}

fn truncate_summary(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_summary_handles_multibyte() {
        let s = "这是中文测试字符串";
        let result = truncate_summary(s, 8);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 27);
    }

    #[test]
    fn truncate_summary_short_string_unchanged() {
        let s = "hello";
        let result = truncate_summary(s, 60);
        assert_eq!(result, "hello");
    }
}
