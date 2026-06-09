use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::permission::{
    decide_tool_call, load_project_policy, recover_session_grants, GateDecisionKind, GateSource,
};

use super::helpers::{
    append_permission_decision, append_permission_request, append_turn_cancelled,
    append_turn_completed, display_summary, gate_choice, gate_source_name, is_inline_skill_tool,
    now_timestamp, permission_candidate, permission_rule, resolved_tool_available,
};
use super::run::find_tool_definition;
use super::slots::{dispatch_tool_slot, spawn_agent_slot, SlotDispatchArgs};
use super::tool_exec::{execute_tool_call, run_tool_pre_hooks};
use super::types::{
    PendingPermission, PendingRun, PendingStep, PermissionChoice, PermissionRequest,
    QueuedToolCall, StreamingChunkState, UiEvent,
};

/// Maximum times a model.post_response hook can force-continue (exit code 2)
/// before the loop is terminated, preventing token-budget exhaustion.
const MAX_FORCE_CONTINUE: u64 = 3;

fn return_blocked_tool(
    mut pending: PendingRun,
    id: &str,
    tool_name: &str,
    display_summary: &str,
    kind: super::types::ToolKind,
    reason: &str,
) -> Result<PendingStep> {
    let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
    let mut store = EventStore::open(&pending.events_path)?;
    store.append(EventPayload::ToolResult {
        turn: pending.turn,
        ts: now_timestamp()?,
        conversation: None,
        tool_call_id: id.to_string(),
        status: "blocked".to_string(),
        summary: reason.to_string(),
        model_content: String::new(),
        truncated: false,
        files_read: Vec::new(),
        files_changed: Vec::new(),
        commands_run: Vec::new(),
        memory_changed: None,
        structured: Some(blocked.clone()),
    })?;
    pending.record_tool_call(tool_name);
    pending.pending_events.push_back(UiEvent::ToolEnd {
        id: id.to_string(),
        status: "blocked".to_string(),
        summary: reason.to_string(),
        model_content: None,
        result: Some(blocked),
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

fn return_tool_result(
    mut pending: PendingRun,
    id: &str,
    tool_name: &str,
    display_summary: &str,
    kind: super::types::ToolKind,
    status: &str,
    summary: &str,
) -> Result<PendingStep> {
    let mut store = EventStore::open(&pending.events_path)?;
    store.append(EventPayload::ToolResult {
        turn: pending.turn,
        ts: now_timestamp()?,
        conversation: None,
        tool_call_id: id.to_string(),
        status: status.to_string(),
        summary: summary.to_string(),
        model_content: String::new(),
        truncated: false,
        files_read: Vec::new(),
        files_changed: Vec::new(),
        commands_run: Vec::new(),
        memory_changed: None,
        structured: None,
    })?;
    if status == "error" {
        pending.record_tool_error(tool_name);
    } else {
        pending.record_tool_call(tool_name);
    }
    pending.pending_events.push_back(UiEvent::ToolEnd {
        id: id.to_string(),
        status: status.to_string(),
        summary: summary.to_string(),
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
            super::tool_exec::record_plugin_hooks(
                &pending.events_path,
                pending.turn,
                "tool.post_execute",
                &hook_results,
            )?;
        }
    }

    let is_error = result.status == "error";
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
    if is_error {
        pending.record_tool_error(&queued.tool_call.name);
    } else {
        pending.record_tool_call(&queued.tool_call.name);
    }
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
        conversation,
        request_id,
        accumulated_text,
        accumulated_thinking,
        stop_reason,
        tool_calls,
        usage,
        handoff_detector,
        thinking_duration_ms,
        ..
    } = state;

    if let Some(ref u) = usage {
        pending.cumulative.input_tokens += u.input_tokens.unwrap_or(0);
        pending.cumulative.output_tokens += u.output_tokens.unwrap_or(0);
        pending.cumulative.cache_read_input_tokens += u.cache_read_input_tokens.unwrap_or(0);
        pending.cumulative.cache_creation_input_tokens +=
            u.cache_creation_input_tokens.unwrap_or(0);
    }

    pending.thinking_duration_ms += thinking_duration_ms;
    pending.model_request_count += 1;

    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        pending.tool_rounds += 1;
    }
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
            input_tokens_total: usage.as_ref().and_then(|u| {
                let input = u.input_tokens.unwrap_or(0);
                let cache_read = u.cache_read_input_tokens.unwrap_or(0);
                let cache_creation = u.cache_creation_input_tokens.unwrap_or(0);
                let total = input + cache_read + cache_creation;
                u32::try_from(total).ok().filter(|value| *value > 0)
            }),
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
                store.append(EventPayload::Handoff {
                    turn: pending.turn,
                    ts: now_timestamp()?,
                    request_id: request_id.clone(),
                    summary: final_summary,
                    keep_turns: pending.handoff_keep_turns,
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
                super::tool_exec::record_plugin_hooks(
                    &pending.events_path,
                    pending.turn,
                    "model.post_response",
                    &results,
                )?;
                if pending.force_continue_count < MAX_FORCE_CONTINUE
                    && results.iter().any(|r| r.exit_code == 2)
                {
                    pending.force_continue_count += 1;
                    if !pending.hook_context.is_empty() {
                        pending.pending_events.push_back(UiEvent::TextDelta {
                            text: String::new(),
                        });
                        drop(store);
                        return super::provider::call_provider_step(pending).await;
                    }
                }
            }
        }

        if !has_tool_calls {
            drop(store);
            append_turn_completed(&pending.events_path, &conversation, pending.turn)?;
            pending.flush_runtime_logs();
            let total_usage = Some(crate::provider::types::ProviderUsage {
                input_tokens: Some(pending.cumulative.input_tokens),
                output_tokens: Some(pending.cumulative.output_tokens),
                cache_read_input_tokens: Some(pending.cumulative.cache_read_input_tokens),
                cache_creation_input_tokens: Some(pending.cumulative.cache_creation_input_tokens),
            });
            return Ok(PendingStep::Done(
                super::types::RunOutput {
                    session_id: pending.session_id.clone(),
                    conversation: conversation.clone(),
                    text: accumulated_text,
                    usage: total_usage.clone(),
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
                conversation: Some(pending.conversation.as_str().to_string()),
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
    let is_cancelled = {
        let notified = pending.cancel_token.notified();
        tokio::pin!(notified);
        notified.enable()
    };
    if is_cancelled {
        append_turn_cancelled(
            &pending.events_path,
            &pending.conversation,
            pending.turn,
            "user_cancelled",
        )?;
        pending.flush_runtime_logs();
        return Ok(PendingStep::Done(
            super::types::RunOutput {
                session_id: pending.session_id.clone(),
                conversation: pending.conversation.clone(),
                text: String::new(),
                usage: None,
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
                session_dir: pending.events_path.parent().unwrap().to_path_buf(),
                workspace: pending.workspace.clone(),
            },
            None,
            pending.turn,
        ));
    }

    // Drain pending_events from previous inline executions (deny/blocked/skill)
    if let Some(event) = pending.pending_events.pop_front() {
        return Ok(PendingStep::Pending {
            pending: Box::new(pending),
            slot: None,
            event: Some(event),
        });
    }

    if let Some(error) = pending.pending_error.take() {
        pending.flush_runtime_logs();
        return Ok(PendingStep::Failed(error));
    }

    if let Some(mut queued) = pending.queued_tool_calls.pop_front() {
        let id = queued.tool_call.id.clone();
        let summary = queued.display_summary.clone();

        if queued.tool_call.name == "agent" {
            // --- Agent call ---
            let target = queued
                .tool_call
                .args
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let prompt = queued
                .tool_call
                .args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let tier = queued
                .tool_call
                .args
                .get("tier")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned);

            let dispatch = match crate::agent::runtime::prepare_dispatch(
                pending.agent_registry.as_ref(),
                &EventStore::replay(&pending.events_path)?,
                &pending.conversation,
                target,
                prompt,
                tier,
                &id,
            ) {
                Ok(dispatch) => dispatch,
                Err(error) => {
                    return return_tool_result(
                        pending,
                        &id,
                        "agent",
                        &summary,
                        super::types::ToolKind::Simple,
                        "error",
                        &error,
                    );
                }
            };

            if pending.child_session_count >= 2 {
                return return_blocked_tool(
                    pending,
                    &id,
                    "agent",
                    &summary,
                    super::types::ToolKind::Simple,
                    "blocked: maximum agent delegation depth (2) reached",
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

            pending.child_session_count += 1;
            let label = format!(
                "{} · {}",
                dispatch.conversation.as_str(),
                truncate_summary(prompt, 60)
            );
            let slot = spawn_agent_slot(
                id.clone(),
                dispatch,
                label,
                pending.workspace.clone(),
                pending.kuku_home.clone(),
                pending.config.clone(),
                pending.prompts_dir.clone(),
                slot_event_tx,
            );

            let kind = slot.kind.clone();

            pending.record_tool_call("agent");
            return Ok(PendingStep::Pending {
                pending: Box::new(pending),
                slot: Some(slot),
                event: Some(UiEvent::ToolStart {
                    id: id.clone(),
                    tool: "agent".to_string(),
                    summary: summary.clone(),
                    kind,
                }),
            });
        } else if is_inline_skill_tool(&queued.tool_call.name)
            && resolved_tool_available(&pending, &queued.tool_call.name)
        {
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
            return execute_inline_tool(pending, &queued, super::types::ToolKind::Simple).await;
        } else {
            // --- Regular tool call ---
            if let Some(request) = pending.take_resumed_permission_request(&id) {
                pending.queued_tool_calls.push_front(queued);
                return Ok(PendingStep::NeedPermission(Box::new(PendingPermission {
                    pending,
                    request,
                })));
            }

            super::provider::ensure_resolved(&mut pending)?;
            let definition =
                find_tool_definition(&pending, &queued.tool_call.name).ok_or_else(|| {
                    crate::error::Error::InvalidArgument(format!(
                        "unknown tool: {}",
                        queued.tool_call.name
                    ))
                })?;
            let risk = definition.risk.clone();
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
                &risk,
                &candidate,
                &policy,
                &session_grants,
            );

            match decision.kind {
                GateDecisionKind::Ask => {
                    let tc_id = id.clone();
                    let request = PermissionRequest {
                        id: tc_id.clone(),
                        conversation: pending.conversation.clone(),
                        turn: pending.turn,
                        tool_call_id: tc_id,
                        tool: queued.tool_call.name.clone(),
                        risk: risk.clone(),
                        summary: summary.clone(),
                        candidate: candidate.clone(),
                        source: gate_source_name(decision.source).to_string(),
                    };
                    append_permission_request(
                        &pending.events_path,
                        &pending.conversation,
                        pending.turn,
                        &request,
                    )?;
                    pending.queued_tool_calls.push_front(queued);
                    return Ok(PendingStep::NeedPermission(Box::new(PendingPermission {
                        pending,
                        request,
                    })));
                }
                GateDecisionKind::Allow => {
                    if !matches!(decision.source, GateSource::TrustPosture) {
                        let choice = gate_choice(&decision.source);
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

                    let (slot, tool_kind) = dispatch_tool_slot(SlotDispatchArgs {
                        tool_name: queued.tool_call.name.clone(),
                        tool_id: id.clone(),
                        args: queued.tool_call.args.clone(),
                        summary: summary.clone(),
                        workspace: pending.workspace.clone(),
                        kuku_home: pending.kuku_home.clone(),
                        prior_events: prior_events.clone(),
                        event_tx: slot_event_tx,
                        config: pending.config.clone(),
                        catalog: pending.catalog.clone(),
                        events_path: pending.events_path.clone(),
                    });
                    pending.record_tool_call(&queued.tool_call.name);
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
                        &pending.conversation,
                        pending.turn,
                        &PermissionRequest {
                            id: id.clone(),
                            conversation: pending.conversation.clone(),
                            turn: pending.turn,
                            tool_call_id: id.clone(),
                            tool: queued.tool_call.name.clone(),
                            risk: risk.clone(),
                            summary: summary.clone(),
                            candidate,
                            source: gate_source_name(decision.source).to_string(),
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
                    pending.record_tool_denied(&queued.tool_call.name);
                    return execute_inline_tool(pending, &queued, super::types::ToolKind::Simple)
                        .await;
                }
            }
        }
    }

    super::provider::call_provider_step(pending).await
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
