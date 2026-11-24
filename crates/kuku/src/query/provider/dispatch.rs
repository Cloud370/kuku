use std::time::{Duration, Instant};

use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::log::LogLevel;
use crate::provider::types::{
    PreparedProviderRequest, ProviderCorrelation, ProviderFailure, ProviderFailureKind,
    ResolvedProvider,
};
use crate::query::helpers::{append_model_error, append_turn_interrupted, now_timestamp};
use crate::query::tool_exec::record_plugin_hooks;
use crate::query::types::{PendingRun, PendingStep, RequestTiming, StreamingChunkState, UiEvent};

use super::compaction::{self, Outcome as CompactionOutcome};
use super::retry::{
    PreflightError, RetryEvent, RetryPhase, RetryStopReason, preflight_stream, record_retry_event,
    retry_stop_reason, scheduled_retry_delay,
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
    ContextRejected {
        failure: ProviderFailure,
        attempt: u8,
    },
    Cancelled,
}

pub(super) async fn dispatch_provider_step(
    mut pending: PendingRun,
    resolved_config: ResolvedProvider,
    existing_events: Vec<crate::event::StoredEvent>,
    mut frozen_context: Option<crate::query::frozen::ContextCommit>,
) -> Result<PendingStep> {
    run_model_pre_request_hooks(&mut pending).await?;
    let handoff_active = pending.handoff_triggered;
    let correlation = super::request_correlation(&pending, &resolved_config);
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
    let request_prepare_us = prepare_started
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let initial_bytes = prepared.body.as_bytes().len();
    let mut recover_context = frozen_context.is_some() && pending.config.handoff().enabled;
    if let Some(context) = frozen_context
        .as_mut()
        .filter(|context| compaction::needed(&pending, context, &resolved_config))
    {
        match Box::pin(compaction::compact(
            &mut pending,
            context,
            &resolved_config,
            &correlation,
            initial_bytes,
            compaction::Trigger::Pressure,
        ))
        .await?
        {
            CompactionOutcome::Keep => {}
            CompactionOutcome::Failed => recover_context = false,
            CompactionOutcome::Replace(body) => {
                recover_context = false;
                prepared = *body;
            }
            CompactionOutcome::Cancelled => return super::super::step::cancellation_step(pending),
            CompactionOutcome::NewInput => {
                return Ok(PendingStep::Pending {
                    pending: Box::new(pending),
                    slot: None,
                    event: None,
                });
            }
        }
    }
    let mut lead_events = Vec::new();
    let (request_id, outcome) = loop {
        let request_id = format!("req_{}", pending.request_num);
        let request_bytes = prepared.body.as_bytes().len();
        if let Some(limit) = max_request_bytes.filter(|limit| request_bytes > *limit) {
            return Err(crate::Error::InvalidArgument(format!(
                "prepared model request exceeds the configured byte limit ({request_bytes} bytes, limit {limit} bytes)"
            )));
        }
        crate::query::budget::reserve(&pending.query)?;
        if let Some(context) = frozen_context.take() {
            crate::query::frozen::record_prepared(
                &mut pending,
                &request_id,
                &prepared,
                context,
                request_prepare_us,
            )?;
        }
        announce_request(
            &mut pending,
            &resolved_config,
            &request_id,
            &mut lead_events,
        )?;
        let provider_trace = Some(crate::provider::trace::ProviderTraceMetadata {
            kuku_home: pending.kuku_home.clone(),
            session_id: pending.session_id.clone(),
            turn: pending.turn,
            request_id: request_id.clone(),
            logs: pending.config.logs(),
            mode: pending.query.provider_trace,
        });

        let outcome = dispatch_prepared(
            &mut pending,
            &resolved_config,
            &prepared,
            &request_id,
            provider_trace,
            recover_context,
        )
        .await?;
        let DispatchOutcome::ContextRejected { failure, attempt } = outcome else {
            break (request_id, outcome);
        };

        let execution = pending
            .query
            .runtime_execution
            .expect("context rejection belongs to a Session");
        let session = super::runtime_session(&pending, execution)?;
        session.commit_model_facts(pending.conversation.as_str(), execution, [EventPayload::Unknown(serde_json::json!({
            "kind": "runtime.model_rejected", "ts": now_timestamp()?, "agent": pending.conversation.as_str(),
            "turn": pending.turn, "request_id": request_id, "attempt": attempt, "status": failure.status,
            "reason": failure.kind.as_event_kind(),
        }))])?;
        pending.model_request_count += 1;
        let selection = crate::query::frozen::prepare(&mut pending, false)?;
        pending.request_base = Some(selection.request);
        let mut context = selection.commit;
        match Box::pin(compaction::compact(
            &mut pending,
            &mut context,
            &resolved_config,
            &correlation,
            request_bytes,
            compaction::Trigger::ProviderOverflow {
                rejected_request: &request_id,
            },
        ))
        .await?
        {
            CompactionOutcome::Replace(body) => {
                recover_context = false;
                prepared = *body;
                frozen_context = Some(context);
                pending.request_num += 1;
            }
            CompactionOutcome::Keep | CompactionOutcome::Failed => {
                record_retry_event(
                    &pending,
                    RetryEvent {
                        request_id: &request_id,
                        failure: &failure,
                        phase: RetryPhase::Stopped,
                        attempt,
                        failed_attempt: attempt,
                        delay: Duration::ZERO,
                        stop_reason: Some(RetryStopReason::NonRetryable),
                    },
                )?;
                break (request_id, DispatchOutcome::Failure(failure));
            }
            CompactionOutcome::Cancelled => return super::super::step::cancellation_step(pending),
            CompactionOutcome::NewInput => {
                pending.pending_events.extend(lead_events);
                return Ok(PendingStep::Pending {
                    pending: Box::new(pending),
                    slot: None,
                    event: None,
                });
            }
        }
    };

    match outcome {
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
        DispatchOutcome::ContextRejected { .. } => unreachable!(),
        DispatchOutcome::Cancelled => super::super::step::cancellation_step(pending),
        DispatchOutcome::Failure(failure)
            if matches!(failure.kind, ProviderFailureKind::ContextTooLarge)
                && pending.query.runtime.is_none() =>
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

fn announce_request(
    pending: &mut PendingRun,
    resolved: &ResolvedProvider,
    request_id: &str,
    lead_events: &mut Vec<UiEvent>,
) -> Result<()> {
    let provider = resolved.kind.as_str().to_owned();
    let model = resolved.model.clone();
    emit_runtime_log(
        pending,
        LogLevel::Info,
        "runtime.model_request",
        format!("requesting {provider} model {model}"),
        Some(serde_json::json!({"provider": provider, "model": model, "request_id": request_id})),
    )?;
    lead_events.extend(pending.pending_events.drain(..));
    lead_events.push(UiEvent::ModelRequest {
        model,
        provider,
        conversation: pending.conversation.clone(),
        turn: pending.turn,
        request_id: request_id.into(),
        request_ordinal: pending.model_request_count + 1,
    });
    if pending.request_num == 1 {
        lead_events.push(UiEvent::TurnStart { turn: pending.turn });
    }
    Ok(())
}

fn prepare_body(
    pending: &PendingRun,
    resolved: &ResolvedProvider,
    correlation: &ProviderCorrelation,
) -> Result<PreparedProviderRequest> {
    let request = pending
        .request_base
        .as_ref()
        .expect("request base captured");
    prepare_request(pending, resolved, correlation, request)
}

pub(super) fn prepare_request(
    pending: &PendingRun,
    resolved: &ResolvedProvider,
    correlation: &ProviderCorrelation,
    request: &crate::query::request::OwnedRequestBase,
) -> Result<PreparedProviderRequest> {
    let current_user_index = request.current_user_index();
    let request = request.request();
    if pending.query.runtime_execution.is_some() && pending.query.model_adapter.is_none() {
        crate::provider::prepare::prepare_provider_request_with_media(
            resolved,
            &request,
            correlation,
            pending.query.runtime_limits.context_replay_bytes(),
            current_user_index,
            |image| {
                super::super::runtime_tools::runtime_session(pending)?
                    .read_model_image(pending.conversation.as_str(), image)
            },
            |image| {
                super::super::runtime_tools::runtime_session(pending)?
                    .read_input_image(pending.conversation.as_str(), image)
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
    recover_context: bool,
) -> Result<DispatchOutcome> {
    if let Some(execution) = pending.query.runtime_execution {
        super::runtime_session(pending, execution)?.commit_model_facts(
            pending.conversation.as_str(),
            execution,
            [EventPayload::Unknown(serde_json::json!({
                "kind": "runtime.model_dispatched",
                "ts": now_timestamp()?,
                "agent": pending.conversation.as_str(),
                "turn": pending.turn,
                "request_id": request_id,
            }))],
        )?;
    }
    let mut failed_attempt = 1;
    let mut started = Instant::now();
    let mut result = crate::query::adapters::model_stream(
        pending,
        request_id,
        resolved,
        prepared,
        trace.clone(),
    )
    .await;
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
        if recover_context && failure.kind == ProviderFailureKind::ContextTooLarge {
            return Ok(DispatchOutcome::ContextRejected {
                failure,
                attempt: failed_attempt,
            });
        }
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
        result = crate::query::adapters::model_stream(
            pending,
            request_id,
            resolved,
            prepared,
            trace.clone(),
        )
        .await;
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
