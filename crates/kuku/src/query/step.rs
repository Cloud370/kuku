use sha2::Digest;

use crate::context::{
    assemble_context, build_request_provenance, rebuild_history, ContextInput, EnvironmentSource,
    FileSource, HistoryRange, RequestProvenanceInput, ToolRegistryProvenance,
};
use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::notice::{
    build_runtime_notices, compute_context_headroom, render_notice_block, NoticeAssemblyInput,
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
            });
            return Ok(PendingStep::Done(
                super::types::RunOutput {
                    session_id: pending.session_id.clone(),
                    text: accumulated_text,
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

    Ok(PendingStep::Pending(Box::new(pending)))
}

pub(super) async fn advance_pending(mut pending: PendingRun) -> Result<PendingStep> {
    if let Some(saved) = pending.saved_tool_call.take() {
        let result = execute_tool_call(&mut pending, &saved.tool_call).await?;
        return Ok(PendingStep::ToolResultReady {
            pending: Box::new(pending),
            ui_event: UiEvent::ToolResult {
                tool_call_id: saved.tool_call.id.clone(),
                status: result.status,
                summary: result.summary,
                structured: result.structured,
            },
        });
    }

    {
        if let Some(queued_tool_call) = pending.queued_tool_calls.pop_front() {
            if let Some(definition) =
                find_tool_definition(&pending, &queued_tool_call.tool_call.name)
            {
                let candidate = permission_candidate(
                    &pending.kuku_home,
                    &pending.workspace,
                    &queued_tool_call.tool_call.name,
                    &queued_tool_call.tool_call.args,
                );
                let policy = load_project_policy(&pending.policy_path)?;
                let prior_events = EventStore::replay(&pending.events_path)?;
                let session_grants = recover_session_grants(&prior_events);
                let decision = decide_tool_call(
                    &queued_tool_call.tool_call.name,
                    &definition.risk,
                    &candidate,
                    &policy,
                    &session_grants,
                );
                match decision.kind {
                    GateDecisionKind::Ask => {
                        let request = PermissionRequest {
                            id: queued_tool_call.tool_call.id.clone(),
                            tool_call_id: queued_tool_call.tool_call.id.clone(),
                            tool: queued_tool_call.tool_call.name.clone(),
                            risk: definition.risk.clone(),
                            summary: queued_tool_call.display_summary.clone(),
                        };
                        append_permission_request(&pending.events_path, pending.turn, &request)?;
                        return Ok(PendingStep::NeedPermission(Box::new(PendingPermission {
                            pending,
                            queued_tool_call,
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
                                &queued_tool_call.tool_call.id,
                                choice,
                                gate_source_name(decision.source),
                                &permission_rule(
                                    &pending.kuku_home,
                                    &pending.workspace,
                                    &queued_tool_call.tool_call.name,
                                    &queued_tool_call.tool_call.args,
                                ),
                            )?;
                        }
                        pending.saved_tool_call = Some(queued_tool_call);
                        let saved = pending.saved_tool_call.as_ref().unwrap();
                        let tc_id = saved.tool_call.id.clone();
                        let tc_name = saved.tool_call.name.clone();
                        let tc_summary = saved.display_summary.clone();
                        return Ok(PendingStep::ToolCallReady {
                            pending: Box::new(pending),
                            ui_event: UiEvent::ToolCall {
                                tool_call_id: tc_id,
                                tool: tc_name,
                                summary: tc_summary,
                            },
                        });
                    }
                    GateDecisionKind::Deny => {
                        append_permission_request(
                            &pending.events_path,
                            pending.turn,
                            &PermissionRequest {
                                id: queued_tool_call.tool_call.id.clone(),
                                tool_call_id: queued_tool_call.tool_call.id.clone(),
                                tool: queued_tool_call.tool_call.name.clone(),
                                risk: definition.risk.clone(),
                                summary: queued_tool_call.display_summary.clone(),
                            },
                        )?;
                        append_permission_decision(
                            &pending.events_path,
                            pending.turn,
                            &queued_tool_call.tool_call.id,
                            PermissionChoice::Deny,
                            gate_source_name(decision.source),
                            &permission_rule(
                                &pending.kuku_home,
                                &pending.workspace,
                                &queued_tool_call.tool_call.name,
                                &queued_tool_call.tool_call.args,
                            ),
                        )?;
                        let tc_id = queued_tool_call.tool_call.id.clone();
                        let result =
                            execute_tool_call(&mut pending, &queued_tool_call.tool_call).await?;
                        return Ok(PendingStep::ToolResultReady {
                            pending: Box::new(pending),
                            ui_event: UiEvent::ToolResult {
                                tool_call_id: tc_id,
                                status: result.status,
                                summary: result.summary,
                                structured: result.structured,
                            },
                        });
                    }
                }
            }

            pending.saved_tool_call = Some(queued_tool_call);
            let saved = pending.saved_tool_call.as_ref().unwrap();
            let tc_id = saved.tool_call.id.clone();
            let tc_name = saved.tool_call.name.clone();
            let tc_summary = saved.display_summary.clone();
            return Ok(PendingStep::ToolCallReady {
                pending: Box::new(pending),
                ui_event: UiEvent::ToolCall {
                    tool_call_id: tc_id,
                    tool: tc_name,
                    summary: tc_summary,
                },
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
            Some(provider_name),
            Some(model),
        )?;
        append_turn_end(&pending.events_path, pending.turn)?;
        return Err(crate::error::Error::Provider(
            "tool loop exceeded maximum provider requests".to_string(),
        ));
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
    let prelude_snapshot = assembly.snapshot_prelude();
    let mut notice_snapshots: Vec<crate::event::types::ContextMessage> = Vec::new();
    if pending.turn > 1 {
        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: &pending.workspace,
            events: &existing_events,
            context_budget_tier: context_headroom.tier,
        });
        let rendered_notices: Vec<String> = notices.iter().map(render_notice_block).collect();
        notice_snapshots = rendered_notices
            .iter()
            .map(|content| crate::event::types::ContextMessage {
                role: "user".to_string(),
                content: content.clone(),
            })
            .collect();
        for (offset, rendered) in rendered_notices.into_iter().enumerate() {
            assembly.prelude_messages.insert(
                1 + offset,
                crate::context::CanonicalMessage::user_text(rendered),
            );
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
        }))),
        Err(failure) => {
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
            Err(crate::error::Error::Provider(failure.message))
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

    let registry = tool::builtin_registry();
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
