mod context {
    pub use kuku::context::ContextAssembly;
}

#[allow(dead_code)]
#[path = "../src/provider/types.rs"]
mod types;

use context::ContextAssembly;
use serde_json::json;
use types::{
    Provider, ProviderFailure, ProviderFailureKind, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderUsage, ResolvedProvider, SecretString,
};

#[test]
fn provider_enum_converts_to_internal_kind() {
    assert_eq!(
        ProviderKind::from(Provider::Anthropic),
        ProviderKind::Anthropic
    );
    assert_eq!(
        ProviderKind::from(Provider::OpenAiCompatible),
        ProviderKind::OpenAiCompatible
    );
}

#[test]
fn secret_string_redacts_debug_and_display_but_can_be_exposed() {
    let secret = SecretString::new("super-secret-key");

    assert_eq!(secret.expose(), "super-secret-key");
    assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
    assert_eq!(secret.to_string(), "<redacted>");
    assert_eq!(secret, SecretString::new("super-secret-key"));
}

#[test]
fn provider_core_types_store_expected_fields() {
    let assembly = ContextAssembly {
        sources: Vec::new(),
    };
    let resolved = ResolvedProvider {
        kind: ProviderKind::Anthropic,
        model: "claude-sonnet-4-6".to_string(),
        base_url: "https://example.test/v1".to_string(),
        api_key: SecretString::new("sk-ant-123"),
    };
    let request = ProviderRequest {
        assembly: assembly.clone(),
        model: resolved.model.clone(),
        max_output_tokens: Some(1024),
        temperature: Some(0.2),
    };
    let usage = ProviderUsage {
        input_tokens: Some(12),
        output_tokens: Some(34),
    };
    let response = ProviderResponse {
        assistant_text: "hello".to_string(),
        stop_reason: Some("end_turn".to_string()),
        provider_request_id: Some("req_123".to_string()),
        usage: Some(usage.clone()),
    };

    let failures = [
        ProviderFailure {
            kind: ProviderFailureKind::MissingConfig,
            message: "missing config".to_string(),
            status: None,
            provider_request_id: None,
            retryable: false,
        },
        ProviderFailure {
            kind: ProviderFailureKind::Authentication,
            message: "auth".to_string(),
            status: Some(401),
            provider_request_id: Some("req_auth".to_string()),
            retryable: false,
        },
        ProviderFailure {
            kind: ProviderFailureKind::RateLimited,
            message: "rate limited".to_string(),
            status: Some(429),
            provider_request_id: Some("req_rate".to_string()),
            retryable: true,
        },
        ProviderFailure {
            kind: ProviderFailureKind::ContextTooLarge,
            message: "too large".to_string(),
            status: Some(413),
            provider_request_id: Some("req_context".to_string()),
            retryable: false,
        },
        ProviderFailure {
            kind: ProviderFailureKind::InvalidRequest,
            message: "invalid".to_string(),
            status: Some(400),
            provider_request_id: Some("req_invalid".to_string()),
            retryable: false,
        },
        ProviderFailure {
            kind: ProviderFailureKind::ProviderUnavailable,
            message: "unavailable".to_string(),
            status: Some(503),
            provider_request_id: Some("req_unavailable".to_string()),
            retryable: true,
        },
        ProviderFailure {
            kind: ProviderFailureKind::Transport,
            message: "transport".to_string(),
            status: None,
            provider_request_id: None,
            retryable: true,
        },
        ProviderFailure {
            kind: ProviderFailureKind::Parse,
            message: "parse".to_string(),
            status: Some(200),
            provider_request_id: Some("req_parse".to_string()),
            retryable: false,
        },
        ProviderFailure {
            kind: ProviderFailureKind::Unknown,
            message: "unknown".to_string(),
            status: Some(500),
            provider_request_id: Some("req_unknown".to_string()),
            retryable: true,
        },
    ];

    assert_eq!(resolved.kind, ProviderKind::Anthropic);
    assert_eq!(resolved.model, "claude-sonnet-4-6");
    assert_eq!(resolved.base_url, "https://example.test/v1");
    assert_eq!(resolved.api_key.expose(), "sk-ant-123");

    assert_eq!(request.assembly, assembly);
    assert_eq!(request.model, "claude-sonnet-4-6");
    assert_eq!(request.max_output_tokens, Some(1024));
    assert_eq!(request.temperature, Some(0.2));

    assert_eq!(response.assistant_text, "hello");
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(response.provider_request_id.as_deref(), Some("req_123"));
    assert_eq!(response.usage, Some(usage.clone()));
    assert_eq!(
        serde_json::to_value(&usage).unwrap(),
        json!({"input_tokens": 12, "output_tokens": 34})
    );

    assert_eq!(failures.len(), 9);
    assert_eq!(failures[0].kind, ProviderFailureKind::MissingConfig);
    assert_eq!(failures[1].status, Some(401));
    assert!(failures[2].retryable);
    assert_eq!(
        failures[3].provider_request_id.as_deref(),
        Some("req_context")
    );
    assert_eq!(failures[4].message, "invalid");
    assert_eq!(failures[5].kind, ProviderFailureKind::ProviderUnavailable);
    assert_eq!(failures[6].kind, ProviderFailureKind::Transport);
    assert_eq!(failures[7].kind, ProviderFailureKind::Parse);
    assert_eq!(failures[8].kind, ProviderFailureKind::Unknown);
}
