mod common;

use std::path::Path;

use common::{openai_sse_response, restore_env, test_config, TestEnv};
use httpmock::prelude::*;
use kuku::{query, Provider};
use serde_json::Value;

fn trace_contents(home: &Path, session_id: &str) -> String {
    let root = home.join("logs").join("provider-trace");
    let records = std::fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>()
        .join("");
    records
        .lines()
        .filter(|line| serde_json::from_str::<Value>(line).unwrap()["session_id"] == session_id)
        .collect::<Vec<_>>()
        .join("\n")
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

    std::env::remove_var("KUKU_PROVIDER_TRACE");
    let request = server.mock(|when, then| {
        when.method(POST)
            .path("/chat/completions")
            .header("authorization", "Bearer HEADER_SECRET")
            .body_contains("REQUEST_SECRET");
        then.status(200)
            .header("x-fixture", "RESPONSE_HEADER_SECRET")
            .body(concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"REPLY_SECRET\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"index\":0,\"id\":\"ID_SECRET\",\"format\":\"google-gemini-v1\",\"data\":\"CIPHER_SECRET\"}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.text\",\"index\":0,\"id\":\"SECOND_ID_SECRET\",\"signature\":\"SIGNATURE_SECRET\"},{\"id\":\"ID_SECRET\",\"data\":\"MORE_SECRET\"}]}}]}\n\n",
                "data: MALFORMED_SECRET\n\n",
            ));
    });
    for (session, mode) in [
        ("s_preview_trace", kuku::ProviderTraceMode::Auto),
        ("s_trace_diagnostic", kuku::ProviderTraceMode::Diagnostic),
    ] {
        assert!(query("REQUEST_SECRET")
            .session(session)
            .provider(Provider::OpenAiCompatible)
            .model("test-chat")
            .base_url(server.base_url())
            .api_key("HEADER_SECRET")
            .config(test_config())
            .provider_trace(mode)
            .run()
            .await
            .is_err());
    }
    request.assert_hits(2);
    let raw = trace_contents(env.home.path(), "s_trace_diagnostic");
    let auto = trace_contents(env.home.path(), "s_preview_trace");
    assert_eq!(!auto.is_empty(), env!("KUKU_BUILD_IS_PREVIEW") == "true");
    std::env::set_var("KUKU_PROVIDER_TRACE", "0");
    assert!(query("REQUEST_SECRET")
        .session("s_trace_disabled")
        .provider(Provider::OpenAiCompatible)
        .model("test-chat")
        .base_url(server.base_url())
        .api_key("HEADER_SECRET")
        .config(test_config())
        .run()
        .await
        .is_err());
    assert!(trace_contents(env.home.path(), "s_trace_disabled").is_empty());
    assert!(!raw.contains("SECRET"), "{raw}");
    let records = raw
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[0]["direction"], "request");
    assert_eq!(records[1]["status"], 200);
    assert_eq!(records[2]["sequence"], 1);
    let first = &records[3]["reasoning_details"][0];
    let second = &records[4]["reasoning_details"][0];
    assert_eq!(first["index"], second["index"]);
    assert_eq!(first["format"], "google-gemini-v1");
    assert_eq!(second["type"], "reasoning.text");
    assert_ne!(first["id_alias"], second["id_alias"]);
    assert_eq!(
        first["id_alias"],
        records[4]["reasoning_details"][1]["id_alias"]
    );
    assert_eq!(records[6]["failure_kind"], "invalid_response");
    assert_eq!(records[6]["after_sequence"], 4);
    restore_env("KUKU_PROVIDER_TRACE", previous_trace);
}
