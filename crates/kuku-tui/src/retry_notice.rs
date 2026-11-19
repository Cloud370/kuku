use kuku_client::{
    HistoryEntry, RetryFailureKind, RetryMetadata, RetryPhase, RetrySource, RetryStopReason,
};
use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::text::{cap, single_line};

const EXPLANATION_LIMIT: usize = 240;

pub(crate) fn history_metadata(entry: Option<&HistoryEntry>, kind: &str) -> RetryMetadata {
    let mut retry: RetryMetadata = entry
        .and_then(|entry| entry.details.get("retry"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    if retry.source == RetrySource::Unknown {
        retry.source = source_for_kind(kind);
    }
    retry
}

pub(crate) fn live_recovery(event: &Value) -> RetryMetadata {
    serde_json::from_value(json!({
        "source":"model.recovery",
        "phase":"started",
        "attempt":event["attempt"],
        "max_attempts":event["max_attempts"],
        "reason":event["reason"],
        "from_request_id":event["from_request_id"],
        "to_request_id":event["to_request_id"],
    }))
    .unwrap_or_default()
}

pub(crate) fn banner(retry: &RetryMetadata, now: OffsetDateTime) -> String {
    match decision(retry) {
        Some(false) => {
            return format!(
                "{} · {}{}",
                stopped_reason(retry),
                explanation(retry),
                status_detail(retry)
            );
        }
        None => {
            let mut parts = vec!["Retry decision recorded".to_owned(), reason(retry)];
            if let Some(attempt) = attempt_label(retry) {
                parts.push(attempt);
            }
            let mut result = parts.join(" · ");
            result.push_str(&status_detail(retry));
            return result;
        }
        Some(true) => {}
    }

    let mut parts = vec![reason(retry), explanation(retry)];
    if let Some(status) = retry.status {
        parts.push(format!("HTTP {status}"));
    }
    if retry.phase == RetryPhase::Waiting {
        if let Some(wait) = wait_label(retry, now) {
            parts.push(wait);
        }
    }
    if let Some(attempt) = attempt_label(retry) {
        parts.push(attempt);
    }
    parts.push("Esc stop".to_owned());
    parts.join(" · ")
}

pub(crate) fn history_text(retry: &RetryMetadata) -> String {
    match decision(retry) {
        Some(false) => {
            return format!(
                "{} · {}{}",
                stopped_reason(retry),
                explanation(retry),
                status_detail(retry)
            );
        }
        None => {
            let mut parts = vec!["Retry decision recorded".to_owned(), reason(retry)];
            if let Some(attempt) = attempt_label(retry) {
                parts.push(attempt);
            }
            let mut result = parts.join(" · ");
            result.push_str(&status_detail(retry));
            return result;
        }
        Some(true) => {}
    }
    let state = match (retry.source, retry.phase) {
        (RetrySource::Recovery, _) => "Model recovery",
        (_, RetryPhase::Waiting) => "Retry scheduled",
        (_, RetryPhase::Started) => "Retry request started",
        (_, RetryPhase::Stopped | RetryPhase::Unknown) => "Automatic retry decision",
    };
    let mut parts = vec![state.to_owned()];
    if let Some(attempt) = attempt_label(retry) {
        parts.push(attempt);
    }
    parts.push(reason(retry));
    parts.push(explanation(retry));
    let mut result = parts.join(" · ");
    let status = status_detail(retry);
    if !status.is_empty() {
        result.push_str(&status);
    }
    result
}

pub(crate) fn model_error_title(code: Option<&str>) -> &'static str {
    match code {
        Some("authentication") => "Authentication error",
        Some("permission_denied") => "Permission denied",
        Some("quota_exhausted") => "Quota exhausted",
        Some("context_too_large") => "Context too large",
        Some("rate_limited") => "Rate limited",
        Some("provider_unavailable") => "Provider unavailable",
        Some("transport") => "Transport error",
        Some("timeout") => "Request timed out",
        Some("invalid_request") => "Invalid request",
        Some("invalid_response") => "Invalid model response",
        Some("internal") => "Model request failed",
        _ => "Execution error",
    }
}

pub(crate) fn model_error_hint(code: Option<&str>) -> &'static str {
    match code {
        Some("authentication") => "Check credentials via /model.",
        Some("permission_denied") => "Check provider permissions and credentials via /model.",
        Some("quota_exhausted") => "Check provider quota or choose another model.",
        Some("context_too_large") => "Reduce context or change model.",
        Some("rate_limited" | "provider_unavailable" | "transport" | "timeout") => {
            "Wait/check service/network; then Continue."
        }
        _ => "Fix the cause, then Continue.",
    }
}

fn source_for_kind(kind: &str) -> RetrySource {
    match kind {
        "runtime.model_retry" => RetrySource::Provider,
        "model.recovery" => RetrySource::Recovery,
        _ => RetrySource::Unknown,
    }
}

pub(crate) fn decision(retry: &RetryMetadata) -> Option<bool> {
    retry
        .will_retry
        .or_else(|| (retry.phase == RetryPhase::Started).then_some(true))
}

fn reason(retry: &RetryMetadata) -> String {
    match retry.source {
        RetrySource::Recovery => match retry.reason.as_deref() {
            Some("length" | "max_tokens" | "max_output_tokens") => "output limit".to_owned(),
            Some("incomplete") => "incomplete response".to_owned(),
            Some("invalid_response") => "invalid response".to_owned(),
            Some("content_filter" | "refusal") => "filtered response".to_owned(),
            _ => "model response".to_owned(),
        },
        RetrySource::Provider => match retry.failed_kind {
            Some(RetryFailureKind::Authentication) => "Authentication failed".to_owned(),
            Some(RetryFailureKind::PermissionDenied) => "Permission denied".to_owned(),
            Some(RetryFailureKind::QuotaExhausted) => "Quota exhausted".to_owned(),
            Some(RetryFailureKind::ContextTooLarge) => "Context too large".to_owned(),
            Some(RetryFailureKind::InvalidRequest) => "Invalid request".to_owned(),
            Some(RetryFailureKind::InvalidResponse) => "Invalid response".to_owned(),
            Some(RetryFailureKind::RateLimited) => "Rate limited".to_owned(),
            Some(RetryFailureKind::Timeout) => "Request timed out".to_owned(),
            Some(RetryFailureKind::ProviderUnavailable) => "Provider unavailable".to_owned(),
            Some(RetryFailureKind::Transport) => "Transport error".to_owned(),
            Some(RetryFailureKind::Internal | RetryFailureKind::Unknown) | None => {
                "Model request failed".to_owned()
            }
        },
        RetrySource::Unknown => "Automatic retry".to_owned(),
    }
}

fn explanation(retry: &RetryMetadata) -> String {
    if let Some(message) = retry
        .message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return safe(message);
    }
    match retry.source {
        RetrySource::Recovery => "Model recovery adjusted the request".to_owned(),
        RetrySource::Provider => match retry.failed_kind {
            Some(RetryFailureKind::Authentication) => "Check credentials via /model".to_owned(),
            Some(RetryFailureKind::PermissionDenied) => {
                "Check provider permissions via /model".to_owned()
            }
            Some(RetryFailureKind::QuotaExhausted) => {
                "Check provider quota or choose another model".to_owned()
            }
            Some(RetryFailureKind::ContextTooLarge) => "Reduce context or change model".to_owned(),
            Some(
                RetryFailureKind::RateLimited
                | RetryFailureKind::Timeout
                | RetryFailureKind::ProviderUnavailable
                | RetryFailureKind::Transport,
            ) => "Wait/check service/network; then Continue".to_owned(),
            Some(RetryFailureKind::InvalidRequest | RetryFailureKind::InvalidResponse)
            | Some(RetryFailureKind::Internal | RetryFailureKind::Unknown)
            | None => "Fix the cause, then Continue".to_owned(),
        },
        RetrySource::Unknown => "Fix the cause, then Continue".to_owned(),
    }
}

fn stop_reason(reason: Option<RetryStopReason>) -> &'static str {
    match reason {
        Some(RetryStopReason::NonRetryable) => "failure is not retryable",
        Some(RetryStopReason::AttemptLimit) => "attempt limit reached",
        Some(RetryStopReason::RequestBudget) => "request budget reached",
        Some(RetryStopReason::Disabled) => "automatic retries disabled",
        Some(RetryStopReason::StreamStarted) => "Reply already started",
        Some(RetryStopReason::Unknown) | None => "no further automatic retry",
    }
}

fn stopped_reason(retry: &RetryMetadata) -> String {
    if retry.stop_reason == Some(RetryStopReason::StreamStarted) {
        "Reply already started; no automatic retry".to_owned()
    } else {
        format!("No automatic retry · {}", stop_reason(retry.stop_reason))
    }
}

fn attempt_label(retry: &RetryMetadata) -> Option<String> {
    let attempt = retry.attempt?;
    Some(match retry.max_attempts {
        Some(maximum) => format!("attempt {attempt}/{maximum}"),
        None => format!("attempt {attempt}"),
    })
}

fn wait_label(retry: &RetryMetadata, now: OffsetDateTime) -> Option<String> {
    let delay_ms = retry.delay_ms?;
    let scheduled_ms = delay_ms.saturating_add(999) / 1_000;
    let (label, seconds) = match retry.ts.as_deref().and_then(parse_timestamp) {
        Some(timestamp) => {
            let elapsed_ms = now
                .unix_timestamp_nanos()
                .saturating_sub(timestamp.unix_timestamp_nanos())
                .max(0)
                / 1_000_000;
            let elapsed_ms = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
            let remaining_ms = delay_ms.saturating_sub(elapsed_ms);
            ("Retry in", remaining_ms.saturating_add(999) / 1_000)
        }
        None => ("Scheduled wait", scheduled_ms),
    };
    Some(format!("{label} {seconds}s"))
}

fn parse_timestamp(timestamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).ok()
}

fn status_detail(retry: &RetryMetadata) -> String {
    retry
        .status
        .map(|status| format!(" · HTTP {status}"))
        .unwrap_or_default()
}

fn safe(text: &str) -> String {
    let text = single_line(text);
    cap(&text, EXPLANATION_LIMIT).to_owned()
}

#[cfg(test)]
mod tests {
    use kuku_client::{RetryFailureKind, RetryPhase, RetrySource};
    use time::OffsetDateTime;

    use super::{banner, history_text, live_recovery, model_error_hint};
    use kuku_client::{RetryMetadata, RetryStopReason};

    fn timestamp(text: &str) -> OffsetDateTime {
        OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).unwrap()
    }

    #[test]
    fn waiting_countdown_uses_event_timestamp_and_scheduled_delay() {
        let retry = RetryMetadata {
            source: RetrySource::Provider,
            phase: RetryPhase::Waiting,
            attempt: Some(2),
            max_attempts: Some(4),
            failed_kind: Some(RetryFailureKind::RateLimited),
            delay_ms: Some(5_000),
            ts: Some("2025-01-01T00:00:00Z".into()),
            will_retry: Some(true),
            ..RetryMetadata::default()
        };
        assert!(banner(&retry, timestamp("2025-01-01T00:00:00Z")).contains("Retry in 5s"));
        assert!(banner(&retry, timestamp("2025-01-01T00:00:03Z")).contains("Retry in 2s"));
        let missing_timestamp = RetryMetadata {
            ts: None,
            ..retry.clone()
        };
        assert!(
            banner(&missing_timestamp, timestamp("2025-01-01T00:00:00Z"))
                .contains("Scheduled wait 5s")
        );
        assert!(banner(&retry, timestamp("2025-01-01T00:00:08Z")).contains("Retry in 0s"));
    }

    #[test]
    fn retry_history_is_durable_and_stopped_reasons_are_explicit() {
        let stopped = RetryMetadata {
            source: RetrySource::Provider,
            phase: RetryPhase::Stopped,
            attempt: Some(3),
            failed_kind: Some(RetryFailureKind::Transport),
            will_retry: Some(false),
            stop_reason: Some(RetryStopReason::StreamStarted),
            ..RetryMetadata::default()
        };
        let text = history_text(&stopped);
        assert!(text.contains("Reply already started; no automatic retry"));
        assert!(!text.contains("Esc stop"));
        assert!(!text.contains("currently retrying"));
    }

    #[test]
    fn recovery_keeps_its_attempt_semantics_and_request_ids() {
        let retry = live_recovery(&serde_json::json!({
            "attempt":2,"max_attempts":3,"reason":"length",
            "from_request_id":"failed","to_request_id":"recovered"
        }));
        assert_eq!(RetrySource::Recovery, retry.source);
        assert_eq!(RetryPhase::Started, retry.phase);
        assert_eq!(Some(2), retry.attempt);
        assert_eq!(Some(3), retry.max_attempts);
        assert_eq!(Some("failed"), retry.from_request_id.as_deref());
        assert_eq!(Some("recovered"), retry.to_request_id.as_deref());
    }

    #[test]
    fn unknown_phase_without_decision_does_not_claim_retry_or_offer_stop() {
        let retry = RetryMetadata {
            source: RetrySource::Provider,
            phase: RetryPhase::Unknown,
            attempt: Some(3),
            ..RetryMetadata::default()
        };
        let text = banner(&retry, timestamp("2025-01-01T00:00:00Z"));
        assert!(text.contains("Retry decision recorded"));
        assert!(!text.contains("Esc stop"));
        assert!(!text.contains("Retry in"));
    }

    #[test]
    fn auth_and_quota_hints_do_not_suggest_continue() {
        assert!(model_error_hint(Some("authentication")).contains("/model"));
        assert!(!model_error_hint(Some("authentication")).contains("/continue"));
        assert!(!model_error_hint(Some("quota_exhausted")).contains("/continue"));
        assert!(model_error_hint(Some("unknown")).contains("Continue"));
    }
}
