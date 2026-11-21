use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore, ModelStopReason};
use crate::notice::compute_context_headroom;
use crate::provider::trace::ProviderTraceMetadata;
use crate::provider::types::ProviderFailureKind;

use super::helpers::{append_model_error, append_turn_interrupted, now_timestamp};
use super::request::RecoveredRequest;
use super::types::{ModelRecoveryInfo, PendingRun, PendingStep, StreamingChunkState, UiEvent};

const MAX_RECOVERY_ATTEMPTS: u8 = 1;
const RECOVERY_SUFFIX_OVERHEAD_BYTES: usize = 128;

pub(super) async fn schedule(
    pending: PendingRun,
    request_id: String,
    discarded_tool_calls: u64,
    usage: Option<&crate::provider::types::ProviderUsage>,
) -> Result<PendingStep> {
    let mut pending = pending;
    if pending.query.runtime.is_some() {
        return interrupt(pending, "length");
    }
    if pending.recovery_count >= MAX_RECOVERY_ATTEMPTS {
        return interrupt(pending, "length");
    }
    let next_request_num = pending.request_num.checked_add(1).ok_or_else(|| {
        Error::InvalidEventStream("provider request id overflow during recovery".to_string())
    })?;
    let Some(base) = pending.request_base.as_ref() else {
        return interrupt(pending, "length");
    };
    let recovered = match base.recovery_request() {
        Ok(request) => request,
        Err(error) => {
            return fail_recovery(pending, error, "length");
        }
    };
    if let Err(error) = preflight_context(base, &pending, usage) {
        return fail_recovery(pending, error, "context_too_large");
    }

    pending.request_num = next_request_num;
    pending.recovery_count += 1;
    let to_request_id = format!("req_{next_request_num}");
    let failed_output_tokens = usage.and_then(|value| value.output_tokens);
    let asset = base.recovery_asset().clone();
    let info = ModelRecoveryInfo {
        conversation: pending.conversation.clone(),
        turn: pending.turn,
        from_request_id: request_id.clone(),
        to_request_id: to_request_id.clone(),
        reason: ModelStopReason::Length,
        attempt: pending.recovery_count,
        max_attempts: MAX_RECOVERY_ATTEMPTS,
    };
    EventStore::open(&pending.events_path)?.append(EventPayload::ModelRecovery {
        conversation: pending.conversation.as_str().to_string(),
        turn: pending.turn,
        ts: now_timestamp()?,
        from_request_id: request_id,
        to_request_id: to_request_id.clone(),
        reason: ModelStopReason::Length,
        attempt: pending.recovery_count,
        max_attempts: MAX_RECOVERY_ATTEMPTS,
        failed_max_output_tokens: base.max_output_tokens(),
        retry_max_output_tokens: base.max_output_tokens(),
        output_tokens_total: failed_output_tokens,
        discarded_tool_calls,
        notice: asset.text,
        prompt_path: asset.path,
        prompt_hash: asset.hash,
    })?;
    dispatch_recovery(pending, recovered, info).await
}

fn preflight_context(
    base: &super::request::OwnedRequestBase,
    pending: &PendingRun,
    usage: Option<&crate::provider::types::ProviderUsage>,
) -> Result<()> {
    let Some(estimated_input) = usage.and_then(|usage| usage.input_tokens) else {
        return Ok(());
    };
    let estimated_input = u32::try_from(estimated_input).map_err(|_| {
        Error::InvalidEventStream("model input usage exceeds context accounting range".to_string())
    })?;
    let headroom = compute_context_headroom(
        base.context_limit(
            &pending
                .resolved
                .as_ref()
                .expect("resolved runtime exists")
                .config,
        ),
        base.max_output_tokens(),
        Some(estimated_input),
    );
    let suffix_tokens = conservative_suffix_tokens(base.recovery_asset().text.len())?;
    if headroom
        .remaining_input_tokens
        .is_some_and(|remaining| remaining < suffix_tokens)
    {
        return Err(Error::Provider {
            kind: ProviderFailureKind::ContextTooLarge,
            message: "insufficient context headroom for output-limit recovery".to_string(),
            provider: pending
                .resolved
                .as_ref()
                .map(|runtime| runtime.config.kind.as_str().to_string()),
            model: pending
                .resolved
                .as_ref()
                .map(|runtime| runtime.config.model.clone()),
        });
    }
    Ok(())
}

fn conservative_suffix_tokens(text_bytes: usize) -> Result<u32> {
    let suffix_bytes = text_bytes
        .checked_add(RECOVERY_SUFFIX_OVERHEAD_BYTES)
        .ok_or_else(|| Error::InvalidEventStream("recovery suffix size overflow".to_string()))?;
    u32::try_from(suffix_bytes)
        .map_err(|_| Error::InvalidEventStream("recovery suffix is too large".to_string()))
}

async fn dispatch_recovery(
    mut pending: PendingRun,
    request: RecoveredRequest,
    info: ModelRecoveryInfo,
) -> Result<PendingStep> {
    let resolved = pending
        .resolved
        .as_ref()
        .expect("resolved runtime exists")
        .config
        .clone();
    let request_id = info.to_request_id.clone();
    let provider_name = resolved.kind.as_str().to_string();
    let model_name = resolved.model.clone();
    super::provider::emit_runtime_log(
        &mut pending,
        crate::log::LogLevel::Info,
        "runtime.model_recovery",
        format!("retrying {provider_name} model {model_name} after output limit"),
        Some(serde_json::json!({
            "request_id": request_id,
            "from_request_id": info.from_request_id,
            "attempt": info.attempt,
        })),
    )?;
    let mut lead_events: Vec<UiEvent> = pending.pending_events.drain(..).collect();
    lead_events.push(UiEvent::ModelRequest {
        model: model_name,
        provider: provider_name,
        conversation: info.conversation.clone(),
        turn: info.turn,
        request_id: info.to_request_id.clone(),
        request_ordinal: pending.model_request_count + 1,
    });
    lead_events.push(UiEvent::ModelRecovery { info: info.clone() });
    let trace = Some(ProviderTraceMetadata {
        kuku_home: pending.kuku_home.clone(),
        session_id: pending.session_id.clone(),
        turn: pending.turn,
        request_id: info.to_request_id.clone(),
    });
    let provider_request = request.request();
    let handoff_active = pending.handoff_triggered;
    match crate::provider::stream_provider(&resolved, &provider_request, trace).await {
        Ok(stream) => Ok(PendingStep::Streaming(Box::new(StreamingChunkState {
            model_permit: None,
            conversation: pending.conversation.clone(),
            request_id,
            pending,
            stream,
            provider_attempt: 1,
            stream_failure: None,
            accounting: super::types::RequestAccounting {
                pricing: resolved.options.model_pricing.get(&resolved.model).cloned(),
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
            request_timing: None,
            lead_events,
            handoff_detector: handoff_active.then(super::handoff::HandoffDetector::new),
            thinking_start: None,
            thinking_duration_ms: 0,
        }))),
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
            Ok(super::provider::pending_failure_step(
                pending,
                lead_events,
                Error::Provider {
                    kind: failure.kind,
                    message: failure.message,
                    provider: Some(resolved.kind.as_str().to_string()),
                    model: Some(resolved.model),
                },
            ))
        }
    }
}

fn interrupt(mut pending: PendingRun, reason: &str) -> Result<PendingStep> {
    let message = if reason == "length" && pending.recovery_count > 0 {
        let limit = pending
            .request_base
            .as_ref()
            .and_then(super::request::OwnedRequestBase::max_output_tokens)
            .map(|limit| format!("max_output_tokens={limit}"))
            .unwrap_or_else(|| "the provider's output limit".to_string());
        format!(
            "model reached {limit} after recovery attempt {}/{}; the turn remains incomplete",
            pending.recovery_count, MAX_RECOVERY_ATTEMPTS
        )
    } else {
        format!("model output limit reached; recovery unavailable ({reason})")
    };
    append_turn_interrupted(
        &pending.events_path,
        &pending.conversation,
        pending.turn,
        reason,
    )?;
    pending.flush_runtime_logs();
    Ok(PendingStep::Failed(Error::Provider {
        kind: ProviderFailureKind::InvalidRequest,
        message,
        provider: pending
            .resolved
            .as_ref()
            .map(|runtime| runtime.config.kind.as_str().to_string()),
        model: pending
            .resolved
            .as_ref()
            .map(|runtime| runtime.config.model.clone()),
    }))
}

fn fail_recovery(mut pending: PendingRun, error: Error, reason: &str) -> Result<PendingStep> {
    append_turn_interrupted(
        &pending.events_path,
        &pending.conversation,
        pending.turn,
        reason,
    )?;
    pending.flush_runtime_logs();
    Ok(PendingStep::Failed(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_estimate_reserves_one_token_per_byte() {
        assert_eq!(
            u32::try_from(4 + RECOVERY_SUFFIX_OVERHEAD_BYTES).unwrap(),
            conservative_suffix_tokens(4).unwrap()
        );
    }
}
