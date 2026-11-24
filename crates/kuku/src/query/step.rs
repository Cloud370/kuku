use super::helpers::{
    append_turn_cancelled, append_turn_completed, append_turn_interrupted, display_summary,
    now_timestamp,
};
use super::tool_response::{self, ToolResponseError, validate_tool_calls};
use super::types::{PendingRun, PendingStep, QueuedToolCall, StreamingChunkState, UiEvent};
use crate::error::Result;
use crate::event::{EventPayload, EventStore, ModelStopReason};
use crate::log::{LogLevel, LogRecord, LogScope};

/// Maximum times a model.post_response hook can force-continue (exit code 2)
/// before the loop is terminated, preventing token-budget exhaustion.
const MAX_FORCE_CONTINUE: u64 = 3;

pub(super) async fn finish_streaming(mut state: StreamingChunkState) -> Result<PendingStep> {
    state.model_permit.take();
    let StreamingChunkState {
        mut pending,
        conversation,
        request_id,
        accumulated_text,
        accumulated_thinking,
        provider_replay,
        accounting,
        stop_reason,
        mut tool_calls,
        tool_arg_buffers,
        tool_call_completions,
        tool_stream_invalid,
        terminal_stream_invalid,
        stream_ended,
        mut usage,
        request_timing,
        handoff_detector,
        thinking_duration_ms,
        provider_attempt,
        ..
    } = state;

    pending.thinking_duration_ms += thinking_duration_ms;
    pending.model_request_count += 1;

    let has_streamed_tool_calls = !tool_calls.is_empty();
    let discarded_tool_calls = tool_calls.len() as u64;
    let has_streamed_tool_artifacts = has_streamed_tool_calls
        || !tool_arg_buffers.is_empty()
        || !tool_call_completions.is_empty()
        || tool_stream_invalid;
    let validated_tool_calls = has_streamed_tool_artifacts.then(|| {
        validate_tool_calls(
            &mut tool_calls,
            tool_arg_buffers,
            tool_call_completions,
            tool_stream_invalid,
            &pending.admitted_tool_call_ids,
        )
    });
    let declared_tool_use = matches!(stop_reason, Some(ModelStopReason::ToolUse));
    let tool_error = match (&validated_tool_calls, declared_tool_use) {
        (Some(Err(error)), _) => Some(*error),
        (None, true) => Some(ToolResponseError::MissingCalls),
        _ => None,
    };
    let argument_rejection = match tool_error {
        Some(ToolResponseError::InvalidArguments { index, problem })
            if stream_ended && !terminal_stream_invalid && declared_tool_use =>
        {
            let tool = tool_calls
                .iter()
                .find(|call| call.index == index)
                .and_then(|call| {
                    pending
                        .resolved
                        .as_ref()?
                        .registry
                        .iter()
                        .find(|definition| definition.name == call.name)
                });
            Some(EventPayload::Unknown(serde_json::json!({
                "kind": "runtime.tool_arguments_rejected",
                "ts": now_timestamp()?, "agent": pending.conversation.as_str(), "turn": pending.turn,
                "request_id": request_id, "tool_index": index, "tool": tool.map(|definition| &definition.name),
                "problem": problem, "step_attempt": u16::from(pending.tool_argument_retries) + 1,
            })))
        }
        _ => None,
    };
    let regenerate = argument_rejection.is_some() && tool_response::can_regenerate(&pending);
    let final_stop_reason = tool_response::stop_reason(
        stop_reason,
        terminal_stream_invalid,
        validated_tool_calls.as_ref(),
    );
    let is_success = matches!(
        final_stop_reason,
        ModelStopReason::EndTurn | ModelStopReason::ToolUse
    );
    let terminal_measured = stream_ended && !terminal_stream_invalid;
    if pending.query.runtime_execution.is_some() && !terminal_measured {
        if let Some(usage) = &mut usage {
            usage.output_tokens = None;
        }
    }
    let response = EventPayload::ModelResponse {
        conversation: Some(pending.conversation.as_str().to_string()),
        turn: pending.turn,
        ts: now_timestamp()?,
        request_id: request_id.clone(),
        text: accumulated_text.clone(),
        thinking: (!accumulated_thinking.is_empty()).then(|| accumulated_thinking.clone()),
        stop_reason: Some(final_stop_reason.clone()),
        input_tokens_total: usage.as_ref().and_then(|usage| usage.input_tokens),
        output_tokens_total: usage.as_ref().and_then(|usage| usage.output_tokens),
    };
    if let Some(execution) = pending.query.runtime_execution {
        let elapsed_us = request_timing
            .filter(|_| terminal_measured)
            .and_then(|timing| timing.elapsed_us);
        let cost = accounting.cost(usage.as_ref().filter(|_| terminal_measured));
        let measurement = EventPayload::Unknown(serde_json::json!({
            "kind": "runtime.model_usage", "ts": now_timestamp()?,
            "agent": pending.conversation.as_str(), "turn": pending.turn,
            "request_id": request_id, "attempt": provider_attempt, "usage": usage, "elapsed_us": elapsed_us, "cost": cost,
        }));
        super::provider::runtime_session(&pending, execution)?.commit_model_facts(
            pending.conversation.as_str(),
            execution,
            provider_replay
                .into_iter()
                .filter(|_| final_stop_reason != ModelStopReason::InvalidResponse)
                .chain(argument_rejection)
                .chain([measurement, response]),
        )?;
    } else {
        EventStore::open(&pending.events_path)?.append(response)?;
    }

    if let Some(ref u) = usage {
        if let Err(error) = accumulate_usage(&mut pending.cumulative, u) {
            append_turn_interrupted(
                &pending.events_path,
                &conversation,
                pending.turn,
                "usage_overflow",
            )?;
            pending.flush_runtime_logs();
            return Ok(PendingStep::Failed(error));
        }
        persist_runtime_model_usage_log(&mut pending, &request_id, u)?;
    }

    if matches!(final_stop_reason, ModelStopReason::Length) {
        return super::recovery::schedule(
            pending,
            request_id,
            discarded_tool_calls,
            usage.as_ref(),
        )
        .await;
    }

    if !is_success {
        if regenerate {
            return tool_response::regenerate(pending, &request_id);
        }
        let diagnostic = tool_error
            .filter(|_| final_stop_reason == ModelStopReason::InvalidResponse)
            .map(ToolResponseError::message);
        let failure = super::provider::record_terminal_stop(
            &mut pending,
            &request_id,
            provider_attempt,
            &final_stop_reason,
            diagnostic,
        )?;
        return Ok(PendingStep::Failed(failure));
    }

    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        pending.tool_rounds += 1;
    }

    {
        let mut store = EventStore::open(&pending.events_path)?;
        if !pending.conversation.is_main() && !accumulated_text.is_empty() {
            store.append(EventPayload::MessageAssistant {
                ts: now_timestamp()?,
                conversation: pending.conversation.as_str().to_string(),
                turn: pending.turn,
                message_id: request_id.clone(),
                text: accumulated_text.clone(),
            })?;
        }

        if let Some(detector) = handoff_detector {
            if let Some(summary) = detector.finish() {
                let trimmed = summary.trim().to_string();
                let final_summary = if trimmed.is_empty() {
                    EventStore::replay(&pending.events_path)?
                        .iter()
                        .rev()
                        .find_map(|e| match &e.payload {
                            EventPayload::MessageUser {
                                conversation, text, ..
                            } if conversation == pending.conversation.as_str() => {
                                Some(text.clone())
                            }
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
                input_tokens: pending.cumulative.input_tokens,
                output_tokens: pending.cumulative.output_tokens,
                cache_read_input_tokens: pending.cumulative.cache_read_input_tokens,
                cache_creation_input_tokens: pending.cumulative.cache_creation_input_tokens,
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

        let ts = now_timestamp()?;
        let facts = tool_calls.iter().map(|tool_call| EventPayload::ToolCall {
            turn: pending.turn,
            ts: ts.clone(),
            conversation: Some(pending.conversation.as_str().to_string()),
            tool_call_id: tool_call.id.clone(),
            request_id: request_id.clone(),
            index: tool_call.index,
            tool: tool_call.name.clone(),
            args: tool_call.args.clone(),
        });
        if let Some(execution) = pending.query.runtime_execution {
            super::provider::runtime_session(&pending, execution)?.commit_model_facts(
                pending.conversation.as_str(),
                execution,
                facts,
            )?;
        } else {
            for fact in facts {
                store.append(fact)?;
            }
        }
    }
    pending.tool_argument_retries = 0;

    for tool_call in tool_calls {
        pending.admitted_tool_call_ids.insert(tool_call.id.clone());
        let display = display_summary(&tool_call.name, &tool_call.args, None);
        pending
            .queued_tool_calls
            .push_back(QueuedToolCall::new(tool_call, display));
    }

    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: None,
    })
}

fn persist_runtime_model_usage_log(
    pending: &mut PendingRun,
    request_id: &str,
    usage: &crate::provider::types::ProviderUsage,
) -> Result<()> {
    let cache_hit_rate = usage
        .cache_input_counts()
        .filter(|(_, input)| *input > 0)
        .map(|(read, input)| read as f64 / input as f64);

    let mut record = LogRecord::new(now_timestamp()?, LogLevel::Info, LogScope::Runtime);
    record.kind = "runtime.model_usage".to_string();
    record.message = format!("model request {request_id} usage");
    record.session_id = Some(pending.session_id.clone());
    record.workspace = Some(pending.workspace.display().to_string());
    record.run_id = Some(pending.session_id.clone());
    record.request_id = Some(request_id.to_string());
    record.turn = Some(pending.turn);
    record.data = Some(serde_json::json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_input_tokens": usage.cache_read_input_tokens,
        "cache_creation_input_tokens": usage.cache_creation_input_tokens,
        "input_tokens_total": usage.input_tokens,
        "cache_hit_rate": cache_hit_rate,
    }));
    let _ = pending.runtime_log_writer.push(record);
    Ok(())
}

pub(super) fn accumulate_usage(
    cumulative: &mut super::types::CumulativeUsage,
    usage: &crate::provider::types::ProviderUsage,
) -> Result<()> {
    add_usage_total(&mut cumulative.input_tokens, usage.input_tokens)?;
    add_usage_total(&mut cumulative.output_tokens, usage.output_tokens)?;
    add_usage_total(
        &mut cumulative.cache_read_input_tokens,
        usage.cache_read_input_tokens,
    )?;
    add_usage_total(
        &mut cumulative.cache_creation_input_tokens,
        usage.cache_creation_input_tokens,
    )
}

fn add_usage_total(total: &mut Option<u64>, value: Option<u64>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    *total =
        Some(total.unwrap_or(0).checked_add(value).ok_or_else(|| {
            crate::error::Error::InvalidEventStream("model usage overflow".into())
        })?);
    Ok(())
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
        return cancellation_step(pending);
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

    if let Some(queued) = pending.queued_tool_calls.pop_front() {
        return super::runtime_tools::advance(pending, queued, slot_event_tx, active_slot_count)
            .await;
    }

    if pending.query.runtime.is_some() && super::runtime_tools::should_yield(&pending)? {
        return super::runtime_tools::yield_step(pending);
    }
    super::provider::call_provider_step(pending).await
}

pub(super) fn cancellation_step(mut pending: PendingRun) -> Result<PendingStep> {
    append_turn_cancelled(
        &pending.events_path,
        &pending.conversation,
        pending.turn,
        "user_cancelled",
    )?;
    pending.flush_runtime_logs();
    Ok(PendingStep::Done(
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
    ))
}
