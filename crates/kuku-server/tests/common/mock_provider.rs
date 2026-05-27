use httpmock::prelude::*;
use httpmock::MockServer;
use serde_json::Value;

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

pub fn anthropic_sse_response(msg: Value) -> String {
    let id = msg
        .get("id")
        .cloned()
        .unwrap_or(Value::String("msg_1".into()));
    let model = msg
        .get("model")
        .cloned()
        .unwrap_or(Value::String("test-model".into()));
    let stop_reason = msg
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    let usage = msg
        .get("usage")
        .cloned()
        .unwrap_or(serde_json::json!({"input_tokens": 0, "output_tokens": 0}));
    let content = msg
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut sse = String::new();

    sse.push_str(&format!(
        "event: message_start\ndata: {}\n\n",
        serde_json::json!({"type":"message_start","message":{"id":id,"model":model,"content":[],"usage":usage}})
    ));

    for (i, block) in content.iter().enumerate() {
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("text");
        if btype == "text" {
            let text = block.get("text").and_then(Value::as_str).unwrap_or("");
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}})
            ));
            if !text.is_empty() {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":text}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        } else if btype == "tool_use" {
            let tc_id = block.get("id").and_then(Value::as_str).unwrap_or("tc_1");
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":tc_id,"name":name,"input":{}}})
            ));
            let args_str = serde_json::to_string(&input).unwrap_or_default();
            if !args_str.is_empty() && args_str != "{}" {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":args_str}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        }
    }

    sse.push_str(&format!(
        "event: message_delta\ndata: {}\n\n",
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":stop_reason},"usage":{"output_tokens":usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0)}})
    ));

    sse.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");

    sse
}

pub fn make_test_config(mock_port: u16) -> kuku::config::Config {
    use kuku::config::{
        ApiKey, Config, DiscoveryConfig, HandoffConfig, ProviderConfig, ThinkLevel, TierConfig,
    };
    use std::collections::BTreeMap;

    let mut providers = BTreeMap::new();
    providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            format: "anthropic".to_string(),
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
    }
}
