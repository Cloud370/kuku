mod context {
    pub use kuku::context::{
        CanonicalMessage, ContextAssembly, ContextSource, InstructionSource, MemorySource,
        MessageBlock, Role,
    };
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

    #[allow(dead_code)]
    pub mod anthropic {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/anthropic.rs"
        ));
    }

    #[allow(dead_code)]
    pub mod openai_compat {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/openai_compat.rs"
        ));
    }
}

use context::{
    CanonicalMessage, ContextAssembly, ContextSource, InstructionSource, MemorySource,
    MessageBlock, Role,
};
use httpmock::prelude::*;
use provider::anthropic::{
    call as call_anthropic, messages_url, render_body as render_anthropic_body,
};
use provider::openai_compat::{
    call as call_openai_compat, chat_completions_url, render_body as render_openai_body,
};
use provider::types::{
    ProviderFailureKind, ProviderKind, ProviderRequest, ResolvedProvider, SecretString,
};
use serde_json::json;

fn sample_assembly() -> ContextAssembly {
    ContextAssembly {
        sources: vec![
            ContextSource::ProjectInstructions(vec![InstructionSource {
                path: "/workspace/AGENTS.md".to_string(),
                kind: "agents".to_string(),
                content: "follow project instructions".to_string(),
            }]),
            ContextSource::GlobalMemory(MemorySource {
                path: "/home/user/.kuku/memory.md".to_string(),
                content: "remember the user prefers concise answers".to_string(),
            }),
            ContextSource::History(vec![
                CanonicalMessage {
                    role: Role::User,
                    blocks: vec![MessageBlock::Text("hello".to_string())],
                },
                CanonicalMessage {
                    role: Role::Assistant,
                    blocks: vec![MessageBlock::Text("hi there".to_string())],
                },
            ]),
        ],
    }
}

#[test]
fn anthropic_messages_url_normalizes_v1_suffix() {
    assert_eq!(
        messages_url("https://api.anthropic.com"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        messages_url("https://gateway.example/v1/"),
        "https://gateway.example/v1/messages"
    );
}

#[test]
fn anthropic_render_body_uses_messages_api_shape() {
    let body = render_anthropic_body(&ProviderRequest {
        assembly: sample_assembly(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: Some(1024),
        temperature: Some(0.2),
    });

    assert_eq!(body["model"], "claude-sonnet-4-6");
    assert_eq!(body["stream"], false);
    assert_eq!(body["max_tokens"], 1024);
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(body.get("stop").is_none());
    assert!(body["system"]
        .as_str()
        .unwrap()
        .contains("follow project instructions"));
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_call_sends_expected_headers_and_parses_success() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: Some(1024),
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::Anthropic,
        model: request.model.clone(),
        base_url: server.base_url(),
        api_key: SecretString::new("anthropic-test-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .header("x-api-key", "anthropic-test-key")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json_body_partial(
                json!({
                    "model": "claude-sonnet-4-6",
                    "stream": false,
                    "max_tokens": 1024,
                })
                .to_string(),
            );
        then.status(200)
            .header("request-id", "msg_req_123")
            .json_body(json!({
                "type": "message",
                "content": [{"type": "text", "text": "Hello from Anthropic"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 11, "output_tokens": 7}
            }));
    });

    let response = call_anthropic(&provider, &request).await.unwrap();

    mock.assert();
    assert_eq!(response.assistant_text, "Hello from Anthropic");
    assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(response.provider_request_id.as_deref(), Some("msg_req_123"));
    assert_eq!(response.usage.unwrap().input_tokens, Some(11));
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_http_failure_is_normalized() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "claude-sonnet-4-6".to_string(),
        max_output_tokens: None,
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::Anthropic,
        model: request.model.clone(),
        base_url: server.base_url(),
        api_key: SecretString::new("bad-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(401)
            .header("request-id", "msg_req_auth")
            .body("unauthorized");
    });

    let failure = call_anthropic(&provider, &request).await.unwrap_err();

    mock.assert();
    assert_eq!(failure.kind, ProviderFailureKind::Authentication);
    assert_eq!(failure.status, Some(401));
    assert_eq!(failure.provider_request_id.as_deref(), Some("msg_req_auth"));
    assert!(!failure.retryable);
}

#[test]
fn openai_chat_completions_url_appends_path() {
    assert_eq!(
        chat_completions_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_url("https://gateway.example/v1/"),
        "https://gateway.example/v1/chat/completions"
    );
}

#[test]
fn openai_render_body_uses_chat_completions_shape() {
    let body = render_openai_body(&ProviderRequest {
        assembly: sample_assembly(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: Some(2048),
        temperature: Some(0.7),
    });

    assert_eq!(body["model"], "gpt-5.4-mini");
    assert_eq!(body["stream"], false);
    assert_eq!(body["max_tokens"], 2048);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][2]["role"], "user");
    assert!(body.get("max_completion_tokens").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn openai_call_sends_bearer_auth_and_parses_success() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: Some(512),
        temperature: Some(0.4),
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::OpenAiCompatible,
        model: request.model.clone(),
        base_url: format!("{}/v1", server.base_url()),
        api_key: SecretString::new("openai-test-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/chat/completions")
            .header("authorization", "Bearer openai-test-key")
            .header("content-type", "application/json")
            .json_body_partial(
                json!({
                    "model": "gpt-5.4-mini",
                    "stream": false,
                    "max_tokens": 512,
                })
                .to_string(),
            );
        then.status(200)
            .header("x-request-id", "chat_req_456")
            .json_body(json!({
                "choices": [{
                    "message": {"content": "Hello from OpenAI"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 13, "completion_tokens": 8}
            }));
    });

    let response = call_openai_compat(&provider, &request).await.unwrap();

    mock.assert();
    assert_eq!(response.assistant_text, "Hello from OpenAI");
    assert_eq!(response.stop_reason.as_deref(), Some("stop"));
    assert_eq!(
        response.provider_request_id.as_deref(),
        Some("chat_req_456")
    );
    assert_eq!(response.usage.unwrap().output_tokens, Some(8));
}

#[tokio::test(flavor = "current_thread")]
async fn openai_http_failure_is_normalized() {
    let server = MockServer::start();
    let request = ProviderRequest {
        assembly: sample_assembly(),
        model: "gpt-5.4-mini".to_string(),
        max_output_tokens: None,
        temperature: None,
    };
    let provider = ResolvedProvider {
        kind: ProviderKind::OpenAiCompatible,
        model: request.model.clone(),
        base_url: format!("{}/v1", server.base_url()),
        api_key: SecretString::new("bad-key"),
    };

    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(429)
            .header("x-request-id", "chat_req_rate")
            .body("rate limited");
    });

    let failure = call_openai_compat(&provider, &request).await.unwrap_err();

    mock.assert();
    assert_eq!(failure.kind, ProviderFailureKind::RateLimited);
    assert_eq!(failure.status, Some(429));
    assert_eq!(
        failure.provider_request_id.as_deref(),
        Some("chat_req_rate")
    );
    assert!(failure.retryable);
}
