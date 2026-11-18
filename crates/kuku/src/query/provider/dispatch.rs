use std::time::Instant;

use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::log::LogLevel;
use crate::provider::types::{
    PreparedProviderRequest, ProviderFailure, ProviderFailureKind, ResolvedProvider,
};
use crate::query::helpers::{append_model_error, append_turn_interrupted, now_timestamp};
use crate::query::tool_exec::record_plugin_hooks;
use crate::query::types::{PendingRun, PendingStep, RequestTiming, StreamingChunkState, UiEvent};

use super::{emit_runtime_log, pending_failure_step};

const PROVIDER_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

struct ProviderAttempt {
    stream: crate::provider::ProviderChunkStream,
    started: Instant,
}

pub(super) async fn dispatch_provider_step(
    mut pending: PendingRun,
    resolved_config: ResolvedProvider,
    existing_events: Vec<crate::event::StoredEvent>,
    frozen_context: Option<crate::query::frozen::ContextCommit>,
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
    let request = pending
        .request_base
        .as_ref()
        .expect("request base captured")
        .request();
    let correlation = crate::provider::types::ProviderCorrelation {
        session_id: pending.session_id.clone(),
        agent_address: pending.conversation.as_str().into(),
        connection_name: resolved_config.connection_name.clone(),
    };
    let prepare_started = Instant::now();
    let prepared =
        crate::provider::prepare_provider_request(&resolved_config, &request, Some(&correlation))
            .map_err(|failure| crate::Error::Provider {
            kind: failure.kind,
            message: failure.message,
            provider: Some(resolved_config.connection_name.clone()),
            model: Some(resolved_config.model.clone()),
        })?;
    if serde_json::to_vec(&prepared.body)?.len() > pending.query.runtime_limits.max_request_bytes {
        return Err(crate::Error::InvalidArgument(
            "prepared model request exceeds the configured byte limit".into(),
        ));
    }
    let request_prepare_us = prepare_started
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let capacity = match &pending.query.model_capacity {
        Some(capacity) => Some(capacity.clone().acquire_owned().await.map_err(|_| {
            crate::Error::InvalidArgument("session model capacity is closed".into())
        })?),
        None => None,
    };
    super::require_runtime_execution(&pending)?;
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
        Ok(ProviderAttempt { stream, started }) => {
            let conversation = pending.conversation.clone();
            Ok(PendingStep::Streaming(Box::new(StreamingChunkState {
                pending,
                conversation,
                request_id,
                stream,
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

async fn dispatch_prepared(
    pending: &mut PendingRun,
    resolved: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    request_id: &str,
    trace: Option<crate::provider::trace::ProviderTraceMetadata>,
) -> Result<std::result::Result<ProviderAttempt, ProviderFailure>> {
    super::require_runtime_execution(pending)?;
    let started = Instant::now();
    let initial =
        crate::query::adapters::model_stream(pending, resolved, prepared, trace.clone()).await;
    let Err(failure) = &initial else {
        return Ok(initial.map(|stream| ProviderAttempt { stream, started }));
    };
    if pending.query.runtime.is_none()
        || !pending.query.runtime_limits.retry_transient_provider_errors
        || !failure.retryable
        || !matches!(
            failure.kind,
            ProviderFailureKind::Transport | ProviderFailureKind::ProviderUnavailable
        )
    {
        return Ok(initial.map(|stream| ProviderAttempt { stream, started }));
    }
    tokio::time::sleep(PROVIDER_RETRY_DELAY).await;
    super::require_runtime_execution(pending)?;
    if crate::query::budget::reserve(&pending.query).is_err() {
        return Ok(initial.map(|stream| ProviderAttempt { stream, started }));
    }
    let execution = pending
        .query
        .runtime_execution
        .expect("transport retries belong to a session execution");
    let session = super::runtime_session(pending, execution)?;
    let retry = EventPayload::Unknown(serde_json::json!({
        "kind": "runtime.model_retry", "ts": now_timestamp()?,
        "agent": pending.conversation.as_str(), "turn": pending.turn, "request_id": request_id,
        "failed_kind": failure.kind.as_event_kind(), "attempt": 2,
    }));
    session.commit_model_facts(pending.conversation.as_str(), execution, [retry])?;
    let started = Instant::now();
    Ok(
        crate::query::adapters::model_stream(pending, resolved, prepared, trace)
            .await
            .map(|stream| ProviderAttempt { stream, started }),
    )
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
