use httpmock::prelude::*;
use httpmock::MockServer;

pub async fn start_mock_provider() -> MockServer {
    MockServer::start_async().await
}

pub fn mock_text_response(server: &MockServer, text: &str) {
    let text = text.to_string();
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_test")
            .header("connection", "close")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 10}
            })));
    });
}

pub use kuku::test_support::anthropic_sse_response;

pub fn make_test_config(mock_port: u16) -> kuku::config::Config {
    use kuku::config::{
        ApiKey, Config, DiscoveryConfig, HandoffConfig, ProviderConfig, ThinkLevel, TierConfig,
        UpdateConfig,
    };
    use std::collections::BTreeMap;

    let mut providers = BTreeMap::new();
    providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            format: kuku::config::ProviderFormat::Anthropic,
            base_url: format!("http://127.0.0.1:{mock_port}"),
            api_key: ApiKey::Plaintext("test-key".to_string()),
        },
    );

    let mut tiers = BTreeMap::new();
    tiers.insert(
        "balanced".to_string(),
        TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            think: ThinkLevel::Off,
            context_window: 200_000,
            max_output_tokens: 48_000,
            purpose: "balanced".to_string(),
        },
    );

    Config {
        tiers,
        providers,
        default_tier: "balanced".to_string(),
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        logs: kuku::config::LogsConfig::default(),
        plugin: kuku::config::PluginConfig::default(),
        update: UpdateConfig::default(),
    }
}
