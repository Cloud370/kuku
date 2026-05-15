mod context {
    pub use kuku::context::ContextAssembly;
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

use provider::error::classify_http_error;
use provider::types::ProviderFailureKind;

#[test]
fn rate_limit_http_error_is_retryable() {
    let failure = classify_http_error(429, "too many requests");

    assert_eq!(failure.kind, ProviderFailureKind::RateLimited);
    assert!(failure.retryable);
    assert_eq!(failure.status, Some(429));
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
