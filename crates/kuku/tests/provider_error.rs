mod config {
    pub use kuku::config::{
        ApiKey, ProviderOptions, ProviderProfile, ResolvedThinking, ThinkLevel,
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
    #[allow(dead_code)]
    pub mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/types.rs"
        ));
    }

    #[allow(dead_code)]
    pub mod error {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/error.rs"
        ));
    }
}

use std::time::{Duration, UNIX_EPOCH};

use provider::error::{classify_http_error, retry_after};
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
    }
    assert!(classify_http_error(429, r#"{"error":{"code":"rate_limit_exceeded"}}"#).retryable);
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
