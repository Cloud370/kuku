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
        retryable: RETRYABLE_STATUSES.contains(&status),
    }
}

pub(crate) fn transport_error(error: &reqwest::Error) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderFailureKind::Transport,
        message: sanitize_transport_message(error),
        status: error.status().map(|status| status.as_u16()),
        provider_request_id: None,
        retryable: true,
    }
}

pub(crate) fn parse_error(body: &str) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderFailureKind::Parse,
        message: format!(
            "failed to parse provider response: {}",
            truncate_and_sanitize(body, 200)
        ),
        status: None,
        provider_request_id: None,
        retryable: false,
    }
}

pub(crate) fn missing_config_error(detail: impl Into<String>) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderFailureKind::MissingConfig,
        message: detail.into(),
        status: None,
        provider_request_id: None,
        retryable: false,
    }
}

fn sanitize_http_message(status: u16, body: &str) -> String {
    format!("HTTP {status}: {}", truncate_and_sanitize(body, 200))
}

fn sanitize_transport_message(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "request timed out".to_string()
    } else if error.is_connect() {
        "could not connect to provider".to_string()
    } else {
        format!("transport error: {error}")
    }
}

fn truncate_and_sanitize(input: &str, max_len: usize) -> String {
    let sanitized = input
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect::<String>()
        .replace(['\n', '\r', '\t'], " ");

    if sanitized.is_empty() {
        return "<empty body>".to_string();
    }

    let mut truncated = String::new();
    for ch in sanitized.chars().take(max_len) {
        truncated.push(ch);
    }
    if sanitized.chars().count() > max_len {
        truncated.push('…');
    }
    truncated
}
