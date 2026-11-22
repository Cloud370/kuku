use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore, PartialReply};
use crate::provider::types::ProviderFailure;

use super::super::helpers::now_timestamp;
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

    let kind = kind.as_event_kind();
    let turn = streaming.pending.turn;
    let address = streaming.conversation.as_str();
    let ts = now_timestamp()?;
    let facts = [
        EventPayload::ModelError {
            conversation: Some(address.into()),
            turn,
            ts: ts.clone(),
            request_id: streaming.request_id.clone(),
            kind: kind.into(),
            message: message.clone(),
        },
        EventPayload::TurnInterrupted {
            conversation: address.into(),
            turn,
            ts: ts.clone(),
            reason: kind.into(),
            partial_reply: PartialReply::from_text(&streaming.accumulated_text),
        },
    ];
    if let Some(execution) = streaming.pending.query.runtime_execution {
        let usage = EventPayload::Unknown(serde_json::json!({
            "kind": "runtime.model_usage", "ts": ts, "agent": address, "turn": turn,
            "request_id": streaming.request_id, "attempt": streaming.provider_attempt,
            "cost": streaming.accounting.cost(None),
        }));
        return super::super::provider::runtime_session(&streaming.pending, execution)?
            .commit_model_facts(address, execution, std::iter::once(usage).chain(facts));
    }
    EventStore::open(&streaming.pending.events_path)?.append_batch(|_| Ok(Vec::from(facts)))?;
    Ok(())
}
