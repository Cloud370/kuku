mod common;

use std::path::Path;

use common::{openai_sse_response, restore_env, test_config, TestEnv};
use httpmock::prelude::*;
use kuku::{query, Provider};
use serde_json::Value;

fn trace_contents(home: &Path, session_id: &str) -> String {
    let root = home.join("logs").join("provider-trace");
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(format!("{session_id}.jsonl")))
        .find(|path| path.is_file())
        .map(std::fs::read_to_string)
        .transpose()
        .unwrap()
        .unwrap_or_else(|| panic!("provider trace missing for {session_id}"))
}

fn assert_redacted_trace(raw: &str, provider: &str, secret: &str) {
    let records = raw
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();

    assert!(records.iter().any(|record| {
        record["provider"] == provider
            && record["direction"] == "request"
            && record["headers"]["authorization"] == "<redacted>"
    }));
    assert!(records
        .iter()
        .any(|record| record["provider"] == provider && record["direction"] == "event"));
    assert!(records
        .iter()
        .any(|record| record["provider"] == provider && record["direction"] == "response"));
    assert!(!raw.contains(secret));
}

#[tokio::test(flavor = "current_thread")]
async fn openai_adapters_write_redacted_provider_traces() {
    let env = TestEnv::new();
    let previous_trace = std::env::var_os("KUKU_PROVIDER_TRACE");
    std::env::set_var("KUKU_PROVIDER_TRACE", "1");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/chat/completions")
            .header("authorization", "Bearer chat-secret");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(openai_sse_response(serde_json::json!({
                "choices": [{"message": {"content": "chat ok"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 2}
            })));
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/responses")
            .header("authorization", "Bearer responses-secret");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(concat!(
                "event: response.created\n",
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_trace\",\"status\":\"in_progress\"}}\n\n",
                "event: response.output_text.delta\n",
                "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"responses ok\"}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_trace\",\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}}\n\n",
            ));
    });

    query("chat trace")
        .session("s_trace_chat")
        .provider(Provider::OpenAiCompatible)
        .model("test-chat")
        .base_url(server.base_url())
        .api_key("chat-secret")
        .config(test_config())
        .run()
        .await
        .unwrap();

    query("responses trace")
        .session("s_trace_responses")
        .provider(Provider::OpenAiResponses)
        .model("test-responses")
        .base_url(server.base_url())
        .api_key("responses-secret")
        .config(test_config())
        .run()
        .await
        .unwrap();

    assert_redacted_trace(
        &trace_contents(env.home.path(), "s_trace_chat"),
        "openai-compatible",
        "chat-secret",
    );
    assert_redacted_trace(
        &trace_contents(env.home.path(), "s_trace_responses"),
        "openai-responses",
        "responses-secret",
    );

    restore_env("KUKU_PROVIDER_TRACE", previous_trace);
}
