mod common;

use common::{anthropic_sse_response, test_config, TestEnv};
use httpmock::prelude::*;
use kuku::agent::registry::AgentRegistry;
use kuku::config::ApiKey;
use kuku::config::TierConfig;
use kuku::event::{EventPayload, EventStore};
use kuku::{query, Provider};

fn request_body(req: &HttpMockRequest) -> Option<serde_json::Value> {
    let Some(body) = req.body.as_ref() else {
        return None;
    };
    serde_json::from_slice::<serde_json::Value>(body).ok()
}

fn messages(body: &serde_json::Value) -> Vec<&serde_json::Value> {
    body.get("messages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .collect()
}

fn content_blocks(message: &serde_json::Value) -> Vec<&serde_json::Value> {
    match message.get("content") {
        Some(serde_json::Value::Array(blocks)) => blocks.iter().collect(),
        Some(serde_json::Value::String(_)) => vec![message.get("content").unwrap()],
        _ => Vec::new(),
    }
}

fn message_texts<'a>(body: &'a serde_json::Value) -> impl Iterator<Item = &'a str> {
    messages(body)
        .into_iter()
        .flat_map(content_blocks)
        .filter_map(|block| match block {
            serde_json::Value::String(text) => Some(text.as_str()),
            serde_json::Value::Object(object) => {
                object.get("text").and_then(serde_json::Value::as_str)
            }
            _ => None,
        })
}

fn current_user_message_has_raw_input_last(req: &HttpMockRequest, expected: &str) -> bool {
    request_body(req).as_ref().is_some_and(|body| {
        messages(body).into_iter().any(|message| {
            if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
                return false;
            }
            let texts = content_blocks(message)
                .into_iter()
                .filter_map(|block| match block {
                    serde_json::Value::String(text) => Some(text.as_str()),
                    serde_json::Value::Object(object) => {
                        object.get("text").and_then(serde_json::Value::as_str)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            texts.last() == Some(&expected)
                && texts.iter().all(|text| !text.contains("<kuku_turn_frame>"))
        })
    })
}

fn body_has_tool_use(req: &HttpMockRequest, tool_call_id: &str, tool_name: &str) -> bool {
    request_body(req).as_ref().is_some_and(|body| {
        messages(body)
            .into_iter()
            .flat_map(content_blocks)
            .any(|block| {
                block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use")
                    && block.get("id").and_then(serde_json::Value::as_str) == Some(tool_call_id)
                    && block.get("name").and_then(serde_json::Value::as_str) == Some(tool_name)
            })
    })
}

fn body_has_tool_result(req: &HttpMockRequest, tool_call_id: &str) -> bool {
    request_body(req).as_ref().is_some_and(|body| {
        messages(body)
            .into_iter()
            .flat_map(content_blocks)
            .any(|block| {
                block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
                    && block.get("tool_use_id").and_then(serde_json::Value::as_str)
                        == Some(tool_call_id)
            })
    })
}

fn body_text_contains(req: &HttpMockRequest, needle: &str) -> bool {
    request_body(req)
        .as_ref()
        .is_some_and(|body| message_texts(body).any(|text| text.contains(needle)))
}

fn initial_parent_cache_probe(req: &HttpMockRequest) -> bool {
    current_user_message_has_raw_input_last(req, "parent cache probe")
        && !body_has_tool_use(req, "toolu_cache_agent", "agent")
        && !body_has_tool_result(req, "toolu_cache_agent")
}

fn parent_followup_keeps_original_open_conversation_notice(req: &HttpMockRequest) -> bool {
    current_user_message_has_raw_input_last(req, "parent cache probe")
        && body_has_tool_use(req, "toolu_cache_agent", "agent")
        && body_has_tool_result(req, "toolu_cache_agent")
        && body_text_contains(req, "explore: turn 3 completed")
        && !body_text_contains(req, "explore: turn 5 completed")
}

fn next_parent_turn_sees_refreshed_open_conversation_notice(req: &HttpMockRequest) -> bool {
    current_user_message_has_raw_input_last(req, "next parent cache probe")
        && body_text_contains(req, "explore: turn 5 completed")
}

fn initial_child_cache_probe(req: &HttpMockRequest) -> bool {
    current_user_message_has_raw_input_last(req, "child cache probe")
        && !body_has_tool_result(req, "toolu_cache_agent")
}

fn anthro_with_agents(query_text: &str, server: &MockServer) -> query::Query {
    let mut config = test_config();
    config.tiers.insert(
        "light".to_string(),
        TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5".to_string(),
            think: kuku::config::ThinkLevel::Off,
            context_window: 200_000,
            max_output_tokens: 16_000,
            purpose: "light".to_string(),
        },
    );
    let provider = config
        .providers
        .get_mut("anthropic")
        .expect("test config has anthropic provider");
    provider.base_url = server.base_url();
    provider.api_key = ApiKey::Plaintext("test-key".to_string());

    query(query_text)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
        .agents(AgentRegistry::builder().builtins().build())
}

fn seed_session_with_completed_explore_turn(events_path: &std::path::Path, session_id: &str) {
    let mut store = EventStore::open(events_path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-09T00:00:00Z".into(),
            schema_version: 2,
            session_id: session_id.into(),
            created_at: "2026-06-09T00:00:00Z".into(),
            kuku_version: env!("CARGO_PKG_VERSION").into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t0".into(),
            conversation: "main".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
            text: "bootstrap main".into(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t1".into(),
            conversation: "explore".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            ts: "t1".into(),
            conversation: "explore".into(),
            turn: 3,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t1".into(),
            conversation: "explore".into(),
            turn: 3,
            text: "previous explore task".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_previous".into()),
        })
        .unwrap();
    store
        .append(EventPayload::MessageAssistant {
            ts: "t1".into(),
            conversation: "explore".into(),
            turn: 3,
            message_id: "req_previous".into(),
            text: "previous explore answer".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
            ts: "t1".into(),
            conversation: "explore".into(),
            turn: 3,
        })
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_notices_are_stable_within_one_parent_turn() {
    let env = TestEnv::new();
    let session_id = "s_runtime_notice_cache_stability";
    seed_session_with_completed_explore_turn(&env.events_path(session_id), session_id);

    let server = MockServer::start();
    let parent_initial = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(initial_parent_cache_probe);
        then.status(200)
            .header("request-id", "req_parent_initial")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_parent_initial",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "delegate"},
                    {"type": "tool_use", "id": "toolu_cache_agent", "name": "agent", "input": {"to": "explore", "message": "child cache probe"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let child_done = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(initial_child_cache_probe);
        then.status(200)
            .header("request-id", "req_child_done")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_child_done",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "child cache result"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let parent_followup = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(parent_followup_keeps_original_open_conversation_notice);
        then.status(200)
            .header("request-id", "req_parent_followup")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_parent_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "parent cache done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(500).body("unexpected request body");
    });

    let output = anthro_with_agents("parent cache probe", &server)
        .session(session_id)
        .run()
        .await
        .unwrap();

    assert_eq!(output.text, "parent cache done");
    parent_initial.assert_hits(1);
    child_done.assert_hits(1);
    parent_followup.assert_hits(1);

    let refresh_server = MockServer::start();
    let next_parent = refresh_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(next_parent_turn_sees_refreshed_open_conversation_notice);
        then.status(200)
            .header("request-id", "req_next_parent")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_next_parent",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "next parent done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 4}
            })));
    });
    refresh_server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(500).body("unexpected refreshed request body");
    });

    let refreshed = anthro_with_agents("next parent cache probe", &refresh_server)
        .session(session_id)
        .run()
        .await
        .unwrap();

    assert_eq!(refreshed.text, "next parent done");
    next_parent.assert_hits(1);
}
