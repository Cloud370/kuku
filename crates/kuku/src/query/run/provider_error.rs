use crate::error::{Error, Result};
use crate::event::EventPayload;
use crate::provider::types::ProviderFailure;

use super::super::helpers::{append_model_error, append_turn_interrupted, now_timestamp};
use super::super::provider::{record_retry_event, RetryEvent, RetryPhase, RetryStopReason};
use super::super::types::StreamingChunkState;

pub(super) fn record_streaming_provider_error_facts(
    streaming: &StreamingChunkState,
    error: &Error,
) -> Result<()> {
    let Error::Provider { kind, message, .. } = error else {
        return Ok(());
    };
    let fallback_failure = ProviderFailure {
        kind: *kind,
        message: message.clone(),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: kind.is_retryable(),
    };
    let failure = streaming
        .stream_failure
        .as_ref()
        .unwrap_or(&fallback_failure);
    let stop_reason = if failure.retryable && failure.kind.is_retryable() {
        RetryStopReason::StreamStarted
    } else {
        RetryStopReason::NonRetryable
    };
    record_retry_event(
        &streaming.pending,
        RetryEvent {
            request_id: &streaming.request_id,
            failure,
            phase: RetryPhase::Stopped,
            attempt: streaming.provider_attempt,
            failed_attempt: streaming.provider_attempt,
            delay: std::time::Duration::ZERO,
            stop_reason: Some(stop_reason),
        },
    )?;

    let kind_name = kind.as_event_kind();
    let turn = streaming.pending.turn;
    if let Some(execution) = streaming.pending.query.runtime_execution {
        let address = streaming.conversation.as_str();
        return super::super::provider::runtime_session(&streaming.pending, execution)?
            .commit_model_facts(
                address,
                execution,
                [
                    EventPayload::ModelError {
                        conversation: Some(address.into()),
                        turn,
                        ts: now_timestamp()?,
                        request_id: streaming.request_id.clone(),
                        kind: kind_name.into(),
                        message: message.clone(),
                    },
                    EventPayload::TurnInterrupted {
                        conversation: address.into(),
                        turn,
                        ts: now_timestamp()?,
                        reason: kind_name.into(),
                    },
                ],
            );
    }
    let _ = append_model_error(
        &streaming.pending.events_path,
        &streaming.conversation,
        turn,
        streaming.request_id.clone(),
        kind_name,
        message,
    );
    let _ = append_turn_interrupted(
        &streaming.pending.events_path,
        &streaming.conversation,
        turn,
        kind_name,
    );
    Ok(())
}
