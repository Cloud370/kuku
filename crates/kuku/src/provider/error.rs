use std::time::{Duration, SystemTime};

use super::types::{ProviderFailure, ProviderFailureKind};

const QUOTA_ERROR_CODES: &[&str] = &[
    "insufficient_quota",
    "insufficient_balance",
    "credit_balance_too_low",
    "billing_hard_limit_reached",
];

pub(crate) fn classify_http_error(status: u16, body_snippet: &str) -> ProviderFailure {
    let structured_kind = structured_error_kind(body_snippet);
    let kind = match status {
        401 => ProviderFailureKind::Authentication,
        402 => ProviderFailureKind::QuotaExhausted,
        403 => ProviderFailureKind::PermissionDenied,
        413 => ProviderFailureKind::ContextTooLarge,
        429 => {
            structured_permanent_kind(structured_kind).unwrap_or(ProviderFailureKind::RateLimited)
        }
        408 | 409 => structured_permanent_kind(structured_kind)
            .unwrap_or(ProviderFailureKind::ProviderUnavailable),
        500..=599 => structured_permanent_kind(structured_kind)
            .unwrap_or(ProviderFailureKind::ProviderUnavailable),
        400 | 404 | 422 => structured_kind.unwrap_or(ProviderFailureKind::InvalidRequest),
        _ => structured_kind.unwrap_or(ProviderFailureKind::Unknown),
    };

    ProviderFailure {
        kind,
        message: sanitize_http_message(status, body_snippet),
        status: Some(status),
        provider_request_id: None,
        retry_after: None,
        retryable: retryable_http_status(status) && kind.is_retryable(),
    }
}

fn retryable_http_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500..=599)
}

fn structured_permanent_kind(kind: Option<ProviderFailureKind>) -> Option<ProviderFailureKind> {
    kind.filter(|kind| !kind.is_retryable())
}

fn structured_error_kind(body: &str) -> Option<ProviderFailureKind> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return None;
    };
    let error = value.get("error").unwrap_or(&value);
    select_provider_error_code(
        error.get("code").and_then(serde_json::Value::as_str),
        error.get("type").and_then(serde_json::Value::as_str),
    )
    .and_then(provider_error_code_kind)
}

pub(crate) fn select_provider_error_code<'a>(
    code: Option<&'a str>,
    error_type: Option<&'a str>,
) -> Option<&'a str> {
    let code_kind = code.and_then(provider_error_code_kind);
    let type_kind = error_type.and_then(provider_error_code_kind);
    if code_kind.is_some_and(|kind| !kind.is_retryable()) {
        return code;
    }
    if type_kind.is_some_and(|kind| !kind.is_retryable()) {
        return error_type;
    }
    if code_kind.is_some() {
        return code;
    }
    if type_kind.is_some() {
        return error_type;
    }
    code.or(error_type)
}

pub(crate) fn classify_stream_error(code: &str, message: &str) -> ProviderFailure {
    let kind = provider_error_code_kind(code).unwrap_or(ProviderFailureKind::Unknown);
    ProviderFailure {
        kind,
        message: sanitize_provider_message(message),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: kind.is_retryable(),
    }
}

fn provider_error_code_kind(code: &str) -> Option<ProviderFailureKind> {
    let code = code.trim().to_ascii_lowercase();
    if QUOTA_ERROR_CODES.contains(&code.as_str()) {
        return Some(ProviderFailureKind::QuotaExhausted);
    }
    Some(match code.as_str() {
        "authentication_error" | "invalid_api_key" | "unauthorized" => {
            ProviderFailureKind::Authentication
        }
        "permission_error" | "permission_denied" | "access_denied" | "forbidden" => {
            ProviderFailureKind::PermissionDenied
        }
        "rate_limit_error" | "rate_limit_exceeded" | "rate_limited" | "too_many_requests" => {
            ProviderFailureKind::RateLimited
        }
        "timeout" | "timeout_error" | "request_timeout" => ProviderFailureKind::Timeout,
        "context_length_exceeded"
        | "context_window_exceeded"
        | "request_too_large"
        | "prompt_too_large" => ProviderFailureKind::ContextTooLarge,
        "invalid_request" | "invalid_request_error" | "bad_request" => {
            ProviderFailureKind::InvalidRequest
        }
        "invalid_response" | "malformed_response" => ProviderFailureKind::InvalidResponse,
        "server_error"
        | "overloaded_error"
        | "overloaded"
        | "api_error"
        | "service_unavailable" => ProviderFailureKind::ProviderUnavailable,
        _ => return None,
    })
}

fn sanitize_provider_message(message: &str) -> String {
    let mut sanitized = String::new();
    for (index, character) in message.chars().enumerate() {
        if index == 200 {
            sanitized.push('…');
            break;
        }
        sanitized.push(if character.is_control() {
            ' '
        } else {
            character
        });
    }
    if sanitized.is_empty() {
        "provider error".into()
    } else {
        sanitized
    }
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
    let kind = if error.is_timeout() {
        ProviderFailureKind::Timeout
    } else {
        ProviderFailureKind::Transport
    };
    ProviderFailure {
        kind,
        message: sanitize_transport_message(error),
        status: error.status().map(|status| status.as_u16()),
        provider_request_id: None,
        retry_after: None,
        retryable: kind.is_retryable(),
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
