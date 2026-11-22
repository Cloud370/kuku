use std::time::{Duration, Instant};

use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::log::LogLevel;
use crate::provider::types::{
    PreparedProviderRequest, ProviderCorrelation, ProviderFailure, ResolvedProvider,
};
use crate::query::helpers::{append_model_error, append_turn_interrupted, now_timestamp};
use crate::query::tool_exec::record_plugin_hooks;
use crate::query::types::{PendingRun, PendingStep, RequestTiming, StreamingChunkState, UiEvent};

use super::retry::{
    preflight_stream, record_retry_event, retry_stop_reason, scheduled_retry_delay, PreflightError,
    RetryEvent, RetryPhase, RetryStopReason,
};
use super::{emit_runtime_log, pending_failure_step};

struct ProviderAttempt {
    stream: crate::provider::ProviderChunkStream,
    started: Instant,
    provider_attempt: u8,
}

enum DispatchOutcome {
    Attempt(ProviderAttempt),
    Failure(ProviderFailure),
    Cancelled,
}

pub(super) async fn dispatch_provider_step(
    mut pending: PendingRun,
    resolved_config: ResolvedProvider,
    existing_events: Vec<crate::event::StoredEvent>,
    mut frozen_context: Option<crate::query::frozen::ContextCommit>,
) -> Result<PendingStep> {
    run_model_pre_request_hooks(&mut pending).await?;
    let request_id = format!("req_{}", pending.request_num);
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
    let correlation = ProviderCorrelation {
        session_id: pending.session_id.clone(),
        agent_address: pending.conversation.as_str().into(),
        connection_name: resolved_config.connection_name.clone(),
        workspace: pending.workspace.clone(),
    };
    let capacity = match &pending.query.model_capacity {
        Some(capacity) => Some(capacity.clone().acquire_owned().await.map_err(|_| {
            crate::Error::InvalidArgument("session model capacity is closed".into())
        })?),
        None => None,
    };
    super::require_runtime_execution(&pending)?;
    let prepare_started = Instant::now();
    let max_request_bytes = pending.query.runtime_limits.max_request_bytes;
    let mut prepared = prepare_body(&pending, &resolved_config, &correlation)?;
    let initial_bytes = prepared.body.as_bytes().len();
    if initial_bytes > max_request_bytes
        && crate::query::helpers::resolved_tool_available(&pending, "read_output")
    {
        if let Some(context) = &mut frozen_context {
            let reclaim_bytes =
                (initial_bytes - max_request_bytes).saturating_add(max_request_bytes / 8);
            let request = pending
                .request_base
                .as_mut()
                .expect("request base captured");
            if context.reference_output_pages(request, pending.turn, reclaim_bytes)? {
                prepared = prepare_body(&pending, &resolved_config, &correlation)?;
            }
        }
    }
    let request_bytes = prepared.body.as_bytes().len();
    if request_bytes > max_request_bytes {
        return Err(crate::Error::InvalidArgument(format!(
            "prepared model request exceeds the configured byte limit ({request_bytes} bytes, limit {max_request_bytes} bytes)"
        )));
    }
    let request_prepare_us = prepare_started
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    crate::query::budget::reserve(&pending.query)?;
    if let Some(context) = frozen_context {
        crate::query::frozen::record_prepared(
            &mut pending,
            &request_id,
            &prepared,
            context,
            request_prepare_us,
        )?;
    }
    match dispatch_prepared(
        &mut pending,
        &resolved_config,
        &prepared,
        &request_id,
        provider_trace,
    )
    .await?
    {
        DispatchOutcome::Attempt(ProviderAttempt {
            stream,
            started,
            provider_attempt,
        }) => {
            let conversation = pending.conversation.clone();
            Ok(PendingStep::Streaming(Box::new(StreamingChunkState {
                pending,
                conversation,
                request_id,
                stream,
                provider_attempt,
                stream_failure: None,
                accounting: super::super::types::RequestAccounting {
                    pricing: crate::provider::cost::RequestPricing::new(
                        &resolved_config.options,
                        &prepared.model,
                        &prepared.url,
                        prepared.settings.service_tier.as_deref(),
                    ),
                    ..Default::default()
                },
                accumulated_text: String::new(),
                accumulated_thinking: String::new(),
                model_phase: None,
                provider_replay: Vec::new(),
                stop_reason: None,
                tool_calls: Vec::new(),
                tool_arg_buffers: Vec::new(),
                tool_call_completions: Vec::new(),
                tool_stream_invalid: false,
                terminal_stream_invalid: false,
                stream_ended: false,
                provider_request_id: None,
                usage: None,
                request_timing: Some(RequestTiming {
                    started,
                    elapsed_us: None,
                }),
                lead_events,
                handoff_detector: if handoff_active {
                    Some(crate::query::handoff::HandoffDetector::new())
                } else {
                    None
                },
                thinking_start: None,
                thinking_duration_ms: 0,
                model_permit: capacity,
            })))
        }
        DispatchOutcome::Cancelled => super::super::step::cancellation_step(pending),
        DispatchOutcome::Failure(failure)
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
                .unwrap_or_else(|| pending.query.prompt.clone());
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
        DispatchOutcome::Failure(failure) => {
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

fn prepare_body(
    pending: &PendingRun,
    resolved: &ResolvedProvider,
    correlation: &ProviderCorrelation,
) -> Result<PreparedProviderRequest> {
    let request = pending
        .request_base
        .as_ref()
        .expect("request base captured")
        .request();
    if pending.query.runtime_execution.is_some() && pending.query.model_adapter.is_none() {
        crate::provider::prepare::prepare_provider_request_with_media(
            resolved,
            &request,
            correlation,
            pending.query.runtime_limits.max_request_bytes,
            |image| {
                super::super::runtime_tools::runtime_session(pending)?
                    .read_model_image(pending.conversation.as_str(), image)
            },
        )
    } else {
        crate::provider::prepare::prepare_provider_request(resolved, &request, Some(correlation))
    }
    .map_err(|failure| crate::Error::Provider {
        kind: failure.kind,
        message: failure.message,
        provider: Some(resolved.connection_name.clone()),
        model: Some(resolved.model.clone()),
    })
}

async fn dispatch_prepared(
    pending: &mut PendingRun,
    resolved: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    request_id: &str,
    trace: Option<crate::provider::trace::ProviderTraceMetadata>,
) -> Result<DispatchOutcome> {
    super::require_runtime_execution(pending)?;
    let mut failed_attempt = 1;
    let mut started = Instant::now();
    let mut result =
        crate::query::adapters::model_stream(pending, resolved, prepared, trace.clone()).await;
    loop {
        let failure = match result {
            Ok(stream) => match preflight_stream(stream, &pending.cancel_token).await {
                Ok(stream) => {
                    return Ok(DispatchOutcome::Attempt(ProviderAttempt {
                        stream,
                        started,
                        provider_attempt: failed_attempt,
                    }));
                }
                Err(PreflightError::Failure(failure)) => failure,
                Err(PreflightError::Cancelled) => return Ok(DispatchOutcome::Cancelled),
            },
            Err(failure) => failure,
        };
        if pending.query.runtime.is_none() {
            return Ok(DispatchOutcome::Failure(failure));
        }
        let stop_reason = retry_stop_reason(
            pending.query.runtime_limits.retry_transient_provider_errors,
            &failure,
            failed_attempt,
            crate::query::budget::exhausted(&pending.query),
        );
        if let Some(stop_reason) = stop_reason {
            record_retry_event(
                pending,
                RetryEvent {
                    request_id,
                    failure: &failure,
                    phase: RetryPhase::Stopped,
                    attempt: failed_attempt,
                    failed_attempt,
                    delay: Duration::ZERO,
                    stop_reason: Some(stop_reason),
                },
            )?;
            return Ok(DispatchOutcome::Failure(failure));
        }

        let delay = scheduled_retry_delay(failed_attempt, failure.retry_after);
        record_retry_event(
            pending,
            RetryEvent {
                request_id,
                failure: &failure,
                phase: RetryPhase::Waiting,
                attempt: failed_attempt + 1,
                failed_attempt,
                delay,
                stop_reason: None,
            },
        )?;
        tokio::time::sleep(delay).await;
        super::require_runtime_execution(pending)?;
        if crate::query::budget::reserve(&pending.query).is_err() {
            record_retry_event(
                pending,
                RetryEvent {
                    request_id,
                    failure: &failure,
                    phase: RetryPhase::Stopped,
                    attempt: failed_attempt,
                    failed_attempt,
                    delay: Duration::ZERO,
                    stop_reason: Some(RetryStopReason::RequestBudget),
                },
            )?;
            return Ok(DispatchOutcome::Failure(failure));
        }
        let next_attempt = failed_attempt + 1;
        record_retry_event(
            pending,
            RetryEvent {
                request_id,
                failure: &failure,
                phase: RetryPhase::Started,
                attempt: next_attempt,
                failed_attempt,
                delay: Duration::ZERO,
                stop_reason: None,
            },
        )?;
        failed_attempt = next_attempt;
        started = Instant::now();
        result =
            crate::query::adapters::model_stream(pending, resolved, prepared, trace.clone()).await;
    }
}

async fn run_model_pre_request_hooks(pending: &mut PendingRun) -> Result<()> {
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

    Ok(())
}
