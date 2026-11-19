use std::hash::{BuildHasher, Hasher, RandomState};
use std::time::Duration;

use crate::error::Result;
use crate::event::ModelStopReason;
use crate::provider::chunk::ProviderChunk;
use crate::provider::error::classify_stream_error;
use crate::provider::types::{ProviderFailure, ProviderFailureKind};
use crate::provider::ProviderChunkStream;
use crate::query::types::PendingRun;

pub(super) const MAX_PROVIDER_ATTEMPTS: u8 = 6;

const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
const JITTER_SCALE: u128 = 1_000_000;
const MAX_JITTER_UNITS: u32 = 250_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetryPhase {
    Waiting,
    Started,
    Stopped,
}

impl RetryPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Started => "started",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetryStopReason {
    NonRetryable,
    AttemptLimit,
    RequestBudget,
    Disabled,
    StreamStarted,
}

pub(crate) struct RetryEvent<'a> {
    pub(crate) request_id: &'a str,
    pub(crate) failure: &'a ProviderFailure,
    pub(crate) phase: RetryPhase,
    pub(crate) attempt: u8,
    pub(crate) failed_attempt: u8,
    pub(crate) delay: Duration,
    pub(crate) stop_reason: Option<RetryStopReason>,
}

impl RetryStopReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NonRetryable => "non_retryable",
            Self::AttemptLimit => "attempt_limit",
            Self::RequestBudget => "request_budget",
            Self::Disabled => "disabled",
            Self::StreamStarted => "stream_started",
        }
    }
}

pub(super) fn retry_stop_reason(
    enabled: bool,
    failure: &ProviderFailure,
    failed_attempt: u8,
    request_budget_exhausted: bool,
) -> Option<RetryStopReason> {
    if !failure.retryable || !failure.kind.is_retryable() {
        Some(RetryStopReason::NonRetryable)
    } else if !enabled {
        Some(RetryStopReason::Disabled)
    } else if failed_attempt >= MAX_PROVIDER_ATTEMPTS {
        Some(RetryStopReason::AttemptLimit)
    } else if request_budget_exhausted {
        Some(RetryStopReason::RequestBudget)
    } else {
        None
    }
}

pub(super) fn retry_delay(
    failed_attempt: u8,
    retry_after: Option<Duration>,
    jitter_units: u32,
) -> Duration {
    let base_seconds = match failed_attempt {
        0 | 1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => 32,
    };
    let base = Duration::from_secs(base_seconds);
    let jitter_units = u128::from(jitter_units.min(MAX_JITTER_UNITS));
    let jitter_nanos = base.as_nanos() * jitter_units / JITTER_SCALE;
    let local = base + Duration::from_nanos(jitter_nanos as u64);
    retry_after
        .filter(|hint| *hint <= MAX_RETRY_AFTER)
        .map_or(local, |hint| local.max(hint))
}

pub(super) fn scheduled_retry_delay(failed_attempt: u8, retry_after: Option<Duration>) -> Duration {
    let jitter_units = RandomState::new().build_hasher().finish() as u32 % (MAX_JITTER_UNITS + 1);
    retry_delay(failed_attempt, retry_after, jitter_units)
}

pub(crate) fn record_retry_event(pending: &PendingRun, event: RetryEvent<'_>) -> Result<()> {
    let Some(execution) = pending.query.runtime_execution else {
        return Ok(());
    };
    let session = super::runtime_session(pending, execution)?;
    let payload = crate::event::EventPayload::Unknown(serde_json::json!({
        "kind": "runtime.model_retry",
        "ts": crate::query::helpers::now_timestamp()?,
        "agent": pending.conversation.as_str(),
        "turn": pending.turn,
        "request_id": event.request_id,
        "failed_kind": event.failure.kind.as_event_kind(),
        "phase": event.phase.as_str(),
        "failed_attempt": event.failed_attempt,
        "max_attempts": MAX_PROVIDER_ATTEMPTS,
        "attempt": event.attempt,
        "delay_ms": if event.phase == RetryPhase::Waiting {
            event.delay.as_millis() as u64
        } else {
            0
        },
        "will_retry": event.phase != RetryPhase::Stopped,
        "message": event.failure.kind.safe_retry_message(),
        "status": event.failure.status,
        "stop_reason": event.stop_reason.map(RetryStopReason::as_str),
    }));
    session.commit_model_facts(pending.conversation.as_str(), execution, [payload])
}

pub(crate) fn record_terminal_stop(
    pending: &mut PendingRun,
    request_id: &str,
    attempt: u8,
    reason: &ModelStopReason,
) -> Result<crate::error::Error> {
    let kind = match reason {
        ModelStopReason::ContextWindowExceeded => ProviderFailureKind::ContextTooLarge,
        ModelStopReason::InvalidResponse => ProviderFailureKind::InvalidResponse,
        _ => ProviderFailureKind::InvalidRequest,
    };
    let message = format!(
        "model response ended with terminal state {}",
        reason.wire_value()
    );
    record_nonretryable_stream_stop(pending, request_id, attempt, kind, &message)?;
    super::super::helpers::append_turn_interrupted(
        &pending.events_path,
        &pending.conversation,
        pending.turn,
        reason.wire_value(),
    )?;
    pending.flush_runtime_logs();
    Ok(crate::error::Error::Provider {
        kind,
        message,
        provider: None,
        model: None,
    })
}

pub(crate) fn record_nonretryable_stream_stop(
    pending: &PendingRun,
    request_id: &str,
    attempt: u8,
    kind: ProviderFailureKind,
    message: &str,
) -> Result<()> {
    let attempt = attempt.max(1);
    let failure = ProviderFailure {
        kind,
        message: message.to_string(),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: false,
    };
    record_retry_event(
        pending,
        RetryEvent {
            request_id,
            failure: &failure,
            phase: RetryPhase::Stopped,
            attempt,
            failed_attempt: attempt,
            delay: Duration::ZERO,
            stop_reason: Some(RetryStopReason::NonRetryable),
        },
    )
}

pub(super) enum PreflightError {
    Failure(ProviderFailure),
    Cancelled,
}

pub(super) async fn preflight_stream(
    mut stream: ProviderChunkStream,
    cancel_token: &tokio::sync::Notify,
) -> std::result::Result<ProviderChunkStream, PreflightError> {
    use tokio_stream::StreamExt;

    let notified = cancel_token.notified();
    tokio::pin!(notified);
    if notified.as_mut().enable() {
        return Err(PreflightError::Cancelled);
    }
    let mut prefix = Vec::new();
    let mut saw_stream_start = false;
    loop {
        let item = tokio::select! {
            _ = &mut notified => return Err(PreflightError::Cancelled),
            item = stream.next() => item,
        };
        let Some(item) = item else { break };
        match item {
            Err(failure) => return Err(PreflightError::Failure(failure)),
            Ok(ProviderChunk::ServerError { code, message }) => {
                return Err(PreflightError::Failure(classify_stream_error(
                    &code, &message,
                )));
            }
            Ok(chunk) => {
                let is_stream_start = matches!(&chunk, ProviderChunk::StreamStart { .. });
                if is_stream_start && saw_stream_start {
                    return Err(PreflightError::Failure(
                        crate::provider::prepare::invalid_response(
                            "provider emitted duplicate stream-start metadata",
                        ),
                    ));
                }
                prefix.push(Ok(chunk));
                if is_stream_start {
                    saw_stream_start = true;
                } else {
                    break;
                }
            }
        }
    }
    Ok(Box::pin(tokio_stream::iter(prefix).chain(stream)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::ProviderFailureKind;

    fn retryable_failure() -> ProviderFailure {
        ProviderFailure {
            kind: ProviderFailureKind::ProviderUnavailable,
            message: "upstream failed".into(),
            status: Some(503),
            provider_request_id: None,
            retry_after: None,
            retryable: true,
        }
    }

    #[test]
    fn retry_schedule_has_exponential_local_waits_and_bounded_jitter() {
        let expected = [2, 4, 8, 16, 32];
        for (index, seconds) in expected.into_iter().enumerate() {
            let attempt = index as u8 + 1;
            assert_eq!(Duration::from_secs(seconds), retry_delay(attempt, None, 0));
            assert_eq!(
                Duration::from_millis(seconds * 1_250),
                retry_delay(attempt, None, MAX_JITTER_UNITS)
            );
        }
    }

    #[test]
    fn retry_after_only_lengthens_local_delay_when_at_most_thirty_seconds() {
        assert_eq!(
            Duration::from_secs(10),
            retry_delay(1, Some(Duration::from_secs(10)), 0)
        );
        assert_eq!(
            Duration::from_secs(2),
            retry_delay(1, Some(Duration::from_secs(31)), 0)
        );
        assert_eq!(
            Duration::from_secs(30),
            retry_delay(1, Some(Duration::from_secs(30)), 0)
        );
        assert_eq!(
            Duration::from_millis(2_500),
            retry_delay(1, Some(Duration::from_secs(2)), MAX_JITTER_UNITS)
        );
    }

    #[test]
    fn retry_stops_for_disabled_non_retryable_budget_and_attempt_limit() {
        let failure = retryable_failure();
        for failed_attempt in 1..MAX_PROVIDER_ATTEMPTS {
            assert_eq!(
                None,
                retry_stop_reason(true, &failure, failed_attempt, false)
            );
        }
        assert_eq!(
            Some(RetryStopReason::Disabled),
            retry_stop_reason(false, &failure, 1, false)
        );
        assert_eq!(
            Some(RetryStopReason::RequestBudget),
            retry_stop_reason(true, &failure, 1, true)
        );
        assert_eq!(
            Some(RetryStopReason::AttemptLimit),
            retry_stop_reason(true, &failure, MAX_PROVIDER_ATTEMPTS, false)
        );
        let mut permanent = failure;
        permanent.retryable = false;
        assert_eq!(
            Some(RetryStopReason::NonRetryable),
            retry_stop_reason(true, &permanent, 1, false)
        );
    }
}
