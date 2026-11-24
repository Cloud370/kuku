mod config {
    pub use kuku::config::{
        ApiKey, CustomServiceTier, MediaAdapterSupport, ModelThinkingTest, ProviderFormat,
        ProviderOptions, ProviderProfile, RequestFields, RequestedOutput, ResolvedThinking,
        SubscriptionChannel, ThinkLevel,
    };
}

mod error {
    pub use kuku::Result;
}

mod context {
    pub use kuku::context::{CanonicalMessage, ContextAssembly};
}

mod prompt {
    pub use kuku::prompt::PromptCatalog;
}

mod provider {
    #[allow(
        dead_code,
        reason = "this target includes the production modules to exercise provider errors"
    )]
    pub mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/types.rs"
        ));
    }

    #[allow(
        dead_code,
        reason = "this target includes the production modules to exercise provider errors"
    )]
    pub mod error {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/error.rs"
        ));
    }
}

use std::time::{Duration, UNIX_EPOCH};

use provider::error::{classify_http_error, classify_stream_error, retry_after};
use provider::types::ProviderFailureKind;

#[test]
fn rate_limit_http_error_is_retryable() {
    let failure = classify_http_error(429, "too many requests");

    assert_eq!(failure.kind, ProviderFailureKind::RateLimited);
    assert!(failure.retryable);
    assert_eq!(failure.status, Some(429));
}

#[test]
fn exhausted_quota_and_temporary_rate_limit_remain_distinct() {
    for body in [
        r#"{"error":{"code":"insufficient_quota"}}"#,
        r#"{"error":{"type":"insufficient_quota","code":429}}"#,
    ] {
        assert!(!classify_http_error(429, body).retryable);
        assert!(!classify_http_error(503, body).retryable);
    }
    assert!(classify_http_error(429, r#"{"error":{"code":"rate_limit_exceeded"}}"#).retryable);
}

#[test]
fn a_chatgpt_plan_limit_links_to_usage_without_retrying() {
    let failure = classify_http_error(
        429,
        r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#,
    );
    assert_eq!(ProviderFailureKind::QuotaExhausted, failure.kind);
    assert!(!failure.retryable);
    assert!(
        failure
            .message
            .contains("https://chatgpt.com/settings/usage")
    );
}

#[test]
fn a_chatgpt_service_tier_rejection_identifies_the_account_and_model_scope() {
    let failure = classify_http_error(
        400,
        r#"{"error":{"code":"subscription_sharing_unsupported_capability","param":"service_tier"}}"#,
    );
    assert_eq!(ProviderFailureKind::InvalidRequest, failure.kind);
    assert!(!failure.retryable);
    assert!(failure.message.contains("this account and model"));
}

#[test]
fn retry_after_accepts_seconds_and_http_dates() {
    let now = UNIX_EPOCH + Duration::from_secs(1_760_000_000);
    let after = Duration::from_secs(12);
    assert_eq!(Some(after), retry_after("12", now));
    assert_eq!(
        Some(after),
        retry_after(&httpdate::fmt_http_date(now + after), now)
    );
    assert_eq!(
        Some(Duration::ZERO),
        retry_after(&httpdate::fmt_http_date(now - after), now)
    );
    assert_eq!(None, retry_after("probably soon", now));
}

#[test]
fn auth_http_error_is_terminal() {
    let failure = classify_http_error(401, "unauthorized");

    assert_eq!(failure.kind, ProviderFailureKind::Authentication);
    assert!(!failure.retryable);
}

#[test]
fn permission_quota_and_timeout_statuses_use_precise_categories() {
    assert_eq!(
        ProviderFailureKind::PermissionDenied,
        classify_http_error(403, "forbidden").kind
    );
    assert_eq!(
        ProviderFailureKind::QuotaExhausted,
        classify_http_error(402, "payment required").kind
    );
    let timeout_status = classify_http_error(408, "request timeout");
    assert_eq!(
        ProviderFailureKind::ProviderUnavailable,
        timeout_status.kind
    );
    assert!(timeout_status.retryable);
    assert!(classify_http_error(409, "conflict").retryable);
    for status in 500..=599 {
        let failure = classify_http_error(status, "temporary outage");
        assert_eq!(ProviderFailureKind::ProviderUnavailable, failure.kind);
        assert!(failure.retryable, "HTTP {status}");
    }
}

#[test]
fn permanent_structured_signals_precede_transient_codes_and_http_statuses() {
    for quota_code in [
        "insufficient_quota",
        "insufficient_balance",
        "credit_balance_too_low",
        "billing_hard_limit_reached",
    ] {
        let body = format!(r#"{{"error":{{"code":"rate_limit_exceeded","type":"{quota_code}"}}}}"#);
        let failure = classify_http_error(503, &body);
        assert_eq!(ProviderFailureKind::QuotaExhausted, failure.kind);
        assert!(!failure.retryable);
    }

    let permanent = classify_http_error(
        500,
        r#"{"error":{"code":"server_error","type":"invalid_request_error"}}"#,
    );
    assert_eq!(ProviderFailureKind::InvalidRequest, permanent.kind);
    assert!(!permanent.retryable);

    let forbidden = classify_http_error(
        403,
        r#"{"error":{"code":"rate_limit_exceeded","type":"server_error"}}"#,
    );
    assert_eq!(ProviderFailureKind::PermissionDenied, forbidden.kind);
    assert!(!forbidden.retryable);
}

#[test]
fn quota_classification_uses_structured_codes_not_free_text() {
    let failure = classify_http_error(
        503,
        r#"{"error":{"message":"insufficient_quota; please retry later"}}"#,
    );
    assert_eq!(ProviderFailureKind::ProviderUnavailable, failure.kind);
    assert!(failure.retryable);
}

#[test]
fn stream_codes_classify_known_categories_without_free_text_fallback() {
    for (code, kind, retryable) in [
        (
            "overloaded_error",
            ProviderFailureKind::ProviderUnavailable,
            true,
        ),
        ("rate_limit_error", ProviderFailureKind::RateLimited, true),
        (
            "insufficient_quota",
            ProviderFailureKind::QuotaExhausted,
            false,
        ),
        (
            "authentication_error",
            ProviderFailureKind::Authentication,
            false,
        ),
        (
            "invalid_response",
            ProviderFailureKind::InvalidResponse,
            false,
        ),
    ] {
        let failure = classify_stream_error(code, "provider error");
        assert_eq!(kind, failure.kind, "{code}");
        assert_eq!(retryable, failure.retryable, "{code}");
    }
    let unknown = classify_stream_error("quota failure: insufficient_quota", "message");
    assert_eq!(ProviderFailureKind::Unknown, unknown.kind);
    assert!(!unknown.retryable);
}

#[test]
fn server_errors_are_retryable() {
    for status in [500, 502, 503, 504] {
        assert!(
            classify_http_error(status, "temporary outage").retryable,
            "{status} should be retryable"
        );
    }
}

#[test]
fn body_snippet_is_truncated_and_sanitized() {
    let failure = classify_http_error(500, &"x".repeat(500));

    assert!(failure.message.starts_with("HTTP 500:"));
    assert!(failure.message.len() <= 220);
}
