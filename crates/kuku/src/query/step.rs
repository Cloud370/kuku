use sha2::Digest;

use crate::context::{
    assemble_context, build_request_provenance, rebuild_history, restore_frozen_prelude,
    ContextInput, EnvironmentSource, FileSource, HistoryRange, RequestProvenanceInput,
    SubagentRegistryProvenance, ToolRegistryProvenance,
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
use crate::prompt::{builtin_handoff_instruction, load_prompt_template};
use crate::provider::config::{resolve_config, ResolveConfigInput};
use crate::tool;

use super::helpers::{
    append_model_error, append_permission_decision, append_permission_request, append_turn_end,
    current_date_string, display_summary, execute_tool_call, gate_source_name, last_input_tokens,
    load_memory_sources, load_project_instruction_sources, now_timestamp, permission_candidate,
    permission_rule, platform_label, provider_failure_kind, provider_format_name,
};
use super::run::find_tool_definition;
use super::slots::{dispatch_tool_slot, spawn_agent_slot};
use super::types::{
    PendingPermission, PendingRun, PendingStep, PermissionChoice, PermissionRequest,
    QueuedToolCall, ResolvedRuntime, StreamingChunkState, UiEvent,
};

const MAX_REQUEST_LOOP: u64 = 20;

struct HookBlockResult {
    reason: String,
    _package: String,
}

struct HookPreResult {
    block: Option<HookBlockResult>,
    args: serde_json::Value,
}

async fn run_tool_pre_hooks(
    pending: &mut PendingRun,
    tool_name: &str,
    tool_args: &serde_json::Value,
    tool_call_id: &str,
) -> Result<HookPreResult> {
    let Some(ref plugin_reg) = pending.plugin_registry else {
        return Ok(HookPreResult {
            block: None,
            args: tool_args.clone(),
        });
    };
    let hooks = plugin_reg.hooks_for(crate::plugin::HookEvent::ToolPreExecute);
    if hooks.is_empty() {
        return Ok(HookPreResult {
            block: None,
            args: tool_args.clone(),
        });
    }
    let input = crate::plugin::executor::HookInput {
        event: "tool.pre_execute".to_string(),
        session_dir: pending
            .events_path
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        extra: serde_json::json!({
            "tool_name": tool_name,
            "tool_args": tool_args,
            "tool_call_id": tool_call_id,
        }),
    };
    let session_dir = pending.events_path.parent().unwrap().to_path_buf();
    let workspace = pending.workspace.clone();
    let results =
        crate::plugin::executor::execute_hooks(hooks, &input, &session_dir, &workspace).await?;
    super::helpers::record_plugin_hooks(
        &pending.events_path,
        pending.turn,
        "tool.pre_execute",
        &results,
    )?;

    let mut updated_args = tool_args.clone();
    for r in &results {
        if r.output.block {
            return Ok(HookPreResult {
                block: Some(HookBlockResult {
                    reason: r.stderr.clone(),
                    _package: r.package_name.clone(),
                }),
                args: updated_args,
            });
        }
        if let Some(ref ctx) = r.output.additional_context {
            pending.hook_context.push(ctx.clone());
        }
        if let Some(ref new_args) = r.output.updated_args {
            updated_args = new_args.clone();
        }
    }

    Ok(HookPreResult {
        block: None,
        args: updated_args,
    })
}

fn return_blocked_tool(
    mut pending: PendingRun,
    id: &str,
    tool_name: &str,
    display_summary: &str,
    kind: super::types::ToolKind,
    reason: &str,
) -> Result<PendingStep> {
    let mut store = EventStore::open(&pending.events_path)?;
    store.append(EventPayload::ToolResult {
        turn: pending.turn,
        ts: now_timestamp()?,
        tool_call_id: id.to_string(),
        status: "blocked".to_string(),
        summary: reason.to_string(),
        model_content: String::new(),
        truncated: false,
        structured: None,
    })?;
    pending.pending_events.push_back(UiEvent::ToolEnd {
        id: id.to_string(),
        status: "blocked".to_string(),
        summary: reason.to_string(),
        model_content: None,
        result: None,
    });
    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: Some(UiEvent::ToolStart {
            id: id.to_string(),
            tool: tool_name.to_string(),
            summary: display_summary.to_string(),
            kind,
        }),
    })
}

async fn execute_inline_tool(
    mut pending: PendingRun,
    queued: &QueuedToolCall,
    kind: super::types::ToolKind,
) -> Result<PendingStep> {
    let result = execute_tool_call(&mut pending, &queued.tool_call).await?;

    if let Some(ref plugin_reg) = pending.plugin_registry {
        let hooks = plugin_reg.hooks_for(crate::plugin::HookEvent::ToolPostExecute);
        if !hooks.is_empty() {
            let input = crate::plugin::executor::HookInput {
                event: "tool.post_execute".to_string(),
                session_dir: pending
                    .events_path
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
                extra: serde_json::json!({
                    "tool_name": queued.tool_call.name,
                    "tool_call_id": queued.tool_call.id,
                    "status": &result.status,
                    "summary": &result.summary,
                }),
            };
            let sd = pending.events_path.parent().unwrap().to_path_buf();
            let ws = pending.workspace.clone();
            let hook_results =
                crate::plugin::executor::execute_hooks(hooks, &input, &sd, &ws).await?;
            for r in &hook_results {
                if let Some(ref ctx) = r.output.additional_context {
                    pending.hook_context.push(ctx.clone());
                }
            }
            super::helpers::record_plugin_hooks(
                &pending.events_path,
                pending.turn,
                "tool.post_execute",
                &hook_results,
            )?;
        }
    }

    let mc = if result.model_content.is_empty() {
        None
    } else {
        Some(result.model_content)
    };
    pending.pending_events.push_back(UiEvent::ToolEnd {
        id: queued.tool_call.id.clone(),
        status: result.status,
        summary: result.summary,
        model_content: mc,
        result: result.structured,
    });
    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: Some(UiEvent::ToolStart {
            id: queued.tool_call.id.clone(),
            tool: queued.tool_call.name.clone(),
            summary: queued.display_summary.clone(),
            kind,
        }),
    })
}

pub(super) async fn finish_streaming(state: StreamingChunkState) -> Result<PendingStep> {
    let StreamingChunkState {
        mut pending,
        request_id,
        accumulated_text,
        accumulated_thinking,
        stop_reason,
        tool_calls,
        usage,
        handoff_detector,
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

        if let Some(detector) = handoff_detector {
            if let Some(summary) = detector.finish() {
                let trimmed = summary.trim().to_string();
                let final_summary = if trimmed.is_empty() {
                    EventStore::replay(&pending.events_path)?
                        .iter()
                        .rev()
                        .find_map(|e| match &e.payload {
                            EventPayload::UserInput { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "handoff summary unavailable".to_string())
                } else {
                    trimmed
                };
                store.append(EventPayload::HandoffTrigger {
                    ts: now_timestamp()?,
                    trigger: crate::event::HandoffTriggerReason::ContextThreshold,
                })?;
                store.append(EventPayload::Handoff {
                    ts: now_timestamp()?,
                    summary: final_summary,
                    kept_turns: pending.handoff_keep_turns,
                })?;
            }
        }

        if let Some(ref plugin_reg) = pending.plugin_registry {
            let hooks = plugin_reg.hooks_for(crate::plugin::HookEvent::ModelPostResponse);
            if !hooks.is_empty() {
                let input = crate::plugin::executor::HookInput {
                    event: "model.post_response".into(),
                    session_dir: pending
                        .events_path
                        .parent()
                        .unwrap()
                        .to_string_lossy()
                        .into(),
                    extra: serde_json::json!({
                        "text": &accumulated_text,
                        "stop_reason": &final_stop_reason,
                    }),
                };
                let sd = pending.events_path.parent().unwrap().to_path_buf();
                let ws = pending.workspace.clone();
                let results =
                    crate::plugin::executor::execute_hooks(hooks, &input, &sd, &ws).await?;
                for r in &results {
                    if r.exit_code == 2 {
                        pending.request_num += 1;
                    }
                    if let Some(ref ctx) = r.output.additional_context {
                        pending.hook_context.push(ctx.clone());
                    }
                }
                super::helpers::record_plugin_hooks(
                    &pending.events_path,
                    pending.turn,
                    "model.post_response",
                    &results,
                )?;
                if pending.request_num < MAX_REQUEST_LOOP
                    && results.iter().any(|r| r.exit_code == 2)
                {
                    // force-continue: inject hook_context and re-enter provider call
                    if !pending.hook_context.is_empty() {
                        pending.hook_context.clear();
                        // Inject into messages via a synthetic user message
                        pending.pending_events.push_back(UiEvent::TextDelta {
                            text: String::new(),
                        });
                        drop(store);
                        return call_provider_step(pending).await;
                    }
                }
            }
        }

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
                    plugin_registry: pending.plugin_registry.clone(),
                    session_dir: pending.events_path.parent().unwrap().to_path_buf(),
                    workspace: pending.workspace.clone(),
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

pub(super) async fn advance_pending(
    mut pending: PendingRun,
    slot_event_tx: tokio::sync::mpsc::Sender<(String, super::types::SlotEvent)>,
    active_slot_count: usize,
) -> Result<PendingStep> {
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
                    plugin_registry: pending.plugin_registry.clone(),
                    session_dir: pending.events_path.parent().unwrap().to_path_buf(),
                    workspace: pending.workspace.clone(),
                },
                None,
                pending.turn,
            ));
        }
    }

    // Drain pending_events from previous inline executions (deny/blocked/skill)
    if let Some(event) = pending.pending_events.pop_front() {
        return Ok(PendingStep::Pending {
            pending: Box::new(pending),
            slot: None,
            event: Some(event),
        });
    }

    if let Some(mut queued) = pending.queued_tool_calls.pop_front() {
        let id = queued.tool_call.id.clone();
        let summary = queued.display_summary.clone();

        if queued.tool_call.name == "agent" {
            // --- Agent call ---
            let name = queued
                .tool_call
                .args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let prompt = queued
                .tool_call
                .args
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let definition = pending
                .subagent_registry
                .as_ref()
                .and_then(|reg| reg.get(name))
                .cloned();

            let Some(definition) = definition else {
                return execute_inline_tool(pending, &queued, super::types::ToolKind::Simple).await;
            };

            if pending.child_session_count >= 2 {
                return return_blocked_tool(
                    pending,
                    &id,
                    "agent",
                    &summary,
                    super::types::ToolKind::Simple,
                    "blocked: maximum subagent depth (2) reached",
                );
            }

            if active_slot_count >= 32 {
                return return_blocked_tool(
                    pending,
                    &id,
                    "agent",
                    &summary,
                    super::types::ToolKind::Simple,
                    "blocked: maximum concurrent slots (32) reached",
                );
            }

            let hook_result =
                run_tool_pre_hooks(&mut pending, "agent", &queued.tool_call.args, &id).await?;
            if let Some(block) = hook_result.block {
                return return_blocked_tool(
                    pending,
                    &id,
                    "agent",
                    &summary,
                    super::types::ToolKind::Simple,
                    &block.reason,
                );
            }

            let child_session_id = format!(
                "child_{}_{}",
                pending.session_id, pending.child_session_count
            );
            pending.child_session_count += 1;
            let label = format!("{} · {}", name, truncate_summary(prompt, 60));

            let parent_dir = pending.events_path.parent().unwrap().to_path_buf();
            let slot = spawn_agent_slot(
                id.clone(),
                name.to_string(),
                prompt.to_string(),
                label,
                definition,
                parent_dir,
                pending.workspace.clone(),
                pending.kuku_home.clone(),
                pending.config.clone(),
                pending.prompts_dir.clone(),
                child_session_id.clone(),
                pending.child_session_count,
                slot_event_tx,
            );

            return Ok(PendingStep::Pending {
                pending: Box::new(pending),
                slot: Some(slot),
                event: Some(UiEvent::ToolStart {
                    id: id.clone(),
                    tool: "agent".to_string(),
                    summary: summary.clone(),
                    kind: super::types::ToolKind::Agent { child_session_id },
                }),
            });
        } else if queued.tool_call.name == "use_skill" {
            let hook_result =
                run_tool_pre_hooks(&mut pending, "use_skill", &queued.tool_call.args, &id).await?;
            if let Some(block) = hook_result.block {
                return return_blocked_tool(
                    pending,
                    &id,
                    "use_skill",
                    &summary,
                    super::types::ToolKind::Simple,
                    &block.reason,
                );
            }
            return execute_inline_tool(pending, &queued, super::types::ToolKind::Simple).await;
        } else {
            // --- Regular tool call ---
            let definition =
                find_tool_definition(&pending, &queued.tool_call.name).ok_or_else(|| {
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
            let policy = load_project_policy(&pending.policy_path)?;
            let prior_events = EventStore::replay(&pending.events_path)?;
            let session_grants = recover_session_grants(&prior_events);
            let decision = decide_tool_call(
                &queued.tool_call.name,
                &definition.risk,
                &candidate,
                &policy,
                &session_grants,
            );

            match decision.kind {
                GateDecisionKind::Ask => {
                    let tc_id = id.clone();
                    let request = PermissionRequest {
                        id: tc_id.clone(),
                        tool_call_id: tc_id,
                        tool: queued.tool_call.name.clone(),
                        risk: definition.risk.clone(),
                        summary: summary.clone(),
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
                            &id,
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

                    let hook_result = run_tool_pre_hooks(
                        &mut pending,
                        &queued.tool_call.name,
                        &queued.tool_call.args,
                        &id,
                    )
                    .await?;
                    if let Some(block) = hook_result.block {
                        return return_blocked_tool(
                            pending,
                            &id,
                            &queued.tool_call.name,
                            &summary,
                            super::types::ToolKind::Simple,
                            &block.reason,
                        );
                    }
                    queued.tool_call.args = hook_result.args;

                    if active_slot_count >= 32 {
                        return return_blocked_tool(
                            pending,
                            &id,
                            &queued.tool_call.name,
                            &summary,
                            super::types::ToolKind::Simple,
                            "blocked: maximum concurrent slots (32) reached",
                        );
                    }

                    let (slot, tool_kind) = dispatch_tool_slot(
                        &queued.tool_call.name,
                        id.clone(),
                        queued.tool_call.args.clone(),
                        summary.clone(),
                        pending.workspace.clone(),
                        pending.kuku_home.clone(),
                        slot_event_tx,
                        pending.config.clone(),
                        pending.catalog.clone(),
                        pending.events_path.clone(),
                    );
                    return Ok(PendingStep::Pending {
                        pending: Box::new(pending),
                        slot: Some(slot),
                        event: Some(UiEvent::ToolStart {
                            id: id.clone(),
                            tool: queued.tool_call.name.clone(),
                            summary: summary.clone(),
                            kind: tool_kind,
                        }),
                    });
                }
                GateDecisionKind::Deny => {
                    append_permission_request(
                        &pending.events_path,
                        pending.turn,
                        &PermissionRequest {
                            id: id.clone(),
                            tool_call_id: id.clone(),
                            tool: queued.tool_call.name.clone(),
                            risk: definition.risk.clone(),
                            summary: summary.clone(),
                        },
                    )?;
                    append_permission_decision(
                        &pending.events_path,
                        pending.turn,
                        &id,
                        PermissionChoice::Deny,
                        gate_source_name(decision.source),
                        &permission_rule(
                            &pending.kuku_home,
                            &pending.workspace,
                            &queued.tool_call.name,
                            &queued.tool_call.args,
                        ),
                    )?;
                    return execute_inline_tool(pending, &queued, super::types::ToolKind::Simple)
                        .await;
                }
            }
        }
    }

    call_provider_step(pending).await
}

async fn call_provider_step(mut pending: PendingRun) -> Result<PendingStep> {
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

    // Freeze prelude on first turn, restore on subsequent turns.
    // Only the first request stores the prelude in events.jsonl; subsequent
    // requests omit it and restore from the first.
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
            let thinking_overhead: u32 = match resolved.config.think_level {
                crate::config::ThinkLevel::Off => 0,
                crate::config::ThinkLevel::Low => 1024,
                crate::config::ThinkLevel::Medium => 4096,
                crate::config::ThinkLevel::High => 16000,
            };
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
            super::helpers::record_plugin_hooks(
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
                Some(crate::query::types::HandoffDetector::new())
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

// Assembles the full request from workspace, config, and session state; each
// parameter reflects a distinct subsystem boundary.
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
    let thinking_overhead: u32 = match resolved_config.think_level {
        crate::config::ThinkLevel::Off => 0,
        crate::config::ThinkLevel::Low => 1024,
        crate::config::ThinkLevel::Medium => 4096,
        crate::config::ThinkLevel::High => 16000,
    };
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
        let thinking_overhead: u32 = match resolved.config.think_level {
            crate::config::ThinkLevel::Off => 0,
            crate::config::ThinkLevel::Low => 1024,
            crate::config::ThinkLevel::Medium => 4096,
            crate::config::ThinkLevel::High => 16000,
        };
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

    build_request_provenance(RequestProvenanceInput {
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
    })
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
