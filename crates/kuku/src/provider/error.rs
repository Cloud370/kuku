use std::time::{Duration, SystemTime};

use super::types::{ProviderFailure, ProviderFailureKind};

const RETRYABLE_STATUSES: &[u16] = &[408, 409, 429, 500, 502, 503, 504];

pub(crate) fn classify_http_error(status: u16, body_snippet: &str) -> ProviderFailure {
    let kind = match status {
        401 | 403 => ProviderFailureKind::Authentication,
        413 => ProviderFailureKind::ContextTooLarge,
        429 => ProviderFailureKind::RateLimited,
        400 | 404 | 422 => ProviderFailureKind::InvalidRequest,
        500..=u16::MAX => ProviderFailureKind::ProviderUnavailable,
        _ => ProviderFailureKind::Unknown,
    };

    ProviderFailure {
        kind,
        message: sanitize_http_message(status, body_snippet),
        status: Some(status),
        provider_request_id: None,
        retry_after: None,
        retryable: RETRYABLE_STATUSES.contains(&status)
            && !(status == 429 && quota_exhausted(body_snippet)),
    }
}

fn quota_exhausted(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let error = &value["error"];
    ["code", "type"]
        .iter()
        .any(|field| error[*field] == "insufficient_quota")
}

pub(crate) fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|time| time.duration_since(now).unwrap_or_default())
}

pub(crate) fn transport_error(error: &wreq::Error) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderFailureKind::Transport,
        message: sanitize_transport_message(error),
        status: error.status().map(|status| status.as_u16()),
        provider_request_id: None,
        retry_after: None,
        retryable: true,
    }
}

fn sanitize_http_message(status: u16, body: &str) -> String {
    format!("HTTP {status}: {}", truncate(body, 200))
}

fn sanitize_transport_message(error: &wreq::Error) -> String {
    if error.is_timeout() {
        "request timed out".to_string()
    } else if error.is_connect() {
        "could not connect to provider".to_string()
    } else {
        "transport error".to_string()
    }
}

fn truncate(input: &str, max_len: usize) -> String {
    if input.is_empty() {
        return "<empty body>".to_string();
    }

    let mut output = input.chars().take(max_len).collect::<String>();
    if input.chars().count() > max_len {
        output.push('…');
    }
    output
}
