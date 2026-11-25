mod common;

use std::path::Path;

use common::{TestEnv, openai_sse_response, test_config};
use httpmock::prelude::*;
use kuku::{Provider, query};
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
    assert!(
        records
            .iter()
            .any(|record| record["provider"] == provider && record["direction"] == "event")
    );
    assert!(
        records
            .iter()
            .any(|record| record["provider"] == provider && record["direction"] == "response")
    );
    assert!(!raw.contains(secret));
}

#[tokio::test(flavor = "current_thread")]
async fn openai_adapters_write_redacted_provider_traces() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(openai_adapters_write_redacted_provider_traces),
    ) else {
        return;
    };

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
        .provider_trace(kuku::ProviderTraceMode::Full)
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
        .provider_trace(kuku::ProviderTraceMode::Full)
        .provider(Provider::OpenAiResponses)
        .model("test-responses")
        .base_url(server.base_url())
        .api_key("responses-secret")
        .config(test_config())
        .run()
        .await
        .unwrap();

    assert_redacted_trace(
        &trace_contents(env.home.as_path(), "s_trace_chat"),
        "openai-compatible",
        "chat-secret",
    );
    assert_redacted_trace(
        &trace_contents(env.home.as_path(), "s_trace_responses"),
        "openai-responses",
        "responses-secret",
    );

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
        ("s_default_trace", kuku::ProviderTraceMode::Auto),
        ("s_trace_diagnostic", kuku::ProviderTraceMode::Diagnostic),
    ] {
        assert!(
            query("REQUEST_SECRET")
                .session(session)
                .provider(Provider::OpenAiCompatible)
                .model("test-chat")
                .base_url(server.base_url())
                .api_key("HEADER_SECRET")
                .config(test_config())
                .provider_trace(mode)
                .run()
                .await
                .is_err()
        );
    }
    request.assert_hits(2);
    let raw = trace_contents(env.home.as_path(), "s_trace_diagnostic");
    let auto = trace_contents(env.home.as_path(), "s_default_trace");
    assert!(auto.contains("REQUEST_SECRET") && auto.contains("RESPONSE_HEADER_SECRET"));
    assert!(auto.contains("REPLY_SECRET"));
    assert!(auto.contains("<redacted>") && !auto.contains("Bearer HEADER_SECRET"));
    assert!(
        query("REQUEST_SECRET")
            .session("s_trace_disabled")
            .provider_trace(kuku::ProviderTraceMode::Off)
            .provider(Provider::OpenAiCompatible)
            .model("test-chat")
            .base_url(server.base_url())
            .api_key("HEADER_SECRET")
            .config(test_config())
            .run()
            .await
            .is_err()
    );
    assert!(trace_contents(env.home.as_path(), "s_trace_disabled").is_empty());
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
}

#[tokio::test(flavor = "current_thread")]
async fn http_failure_trace_keeps_a_bounded_redacted_body_only_in_full_mode() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(http_failure_trace_keeps_a_bounded_redacted_body_only_in_full_mode),
    ) else {
        return;
    };
    const API_KEY: &str = "only-local-credential-12345";
    const DETAIL: &str = "Request path cannot be relayed";
    const RESPONSE_REQUEST_ID: &str = "fixture-400";
    const AFTER_CUTOFF: &str = "beyond-sample-limit";
    let first = format!("{DETAIL}; credential={API_KEY}; ");
    let partial = "only-local-credential";
    let padding = "x".repeat(4096 - first.len() - partial.len());
    let error_body = format!("{first}{padding}{API_KEY} {AFTER_CUTOFF}");
    let server = MockServer::start();
    let request = server.mock(|when, then| {
        when.method(POST).path("/chat/completions");
        then.status(400)
            .header("x-request-id", RESPONSE_REQUEST_ID)
            .body(error_body);
    });
    for (session, mode) in [
        ("s_http_full", kuku::ProviderTraceMode::Full),
        ("s_http_diagnostic", kuku::ProviderTraceMode::Diagnostic),
    ] {
        assert!(
            query("A normal request body")
                .session(session)
                .provider(Provider::OpenAiCompatible)
                .model("test-chat")
                .base_url(server.base_url())
                .api_key(API_KEY)
                .config(test_config())
                .provider_trace(mode)
                .run()
                .await
                .is_err()
        );
    }
    request.assert_hits(2);

    let raw = trace_contents(env.home.as_path(), "s_http_full");
    let records = raw
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let [request, response, error] = records.as_slice() else {
        panic!("expected request, response, and error");
    };
    assert_eq!(request["headers"]["authorization"], "<redacted>");
    assert!(
        request["body"]
            .to_string()
            .contains("A normal request body")
    );
    assert_eq!(response["headers"]["x-request-id"], RESPONSE_REQUEST_ID);
    assert_eq!(response["status"], 400);
    assert_eq!(request["request_id"], response["request_id"]);
    assert_eq!(request["request_id"], error["request_id"]);
    assert_eq!(error["failure_kind"], "invalid_request");
    assert_eq!(error["body_truncated"], true);
    assert!(
        error["body"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{DETAIL}; credential=<redacted>;"))
    );
    assert!(error["body"].as_str().unwrap().ends_with("<redacted>"));
    assert!(!raw.contains(partial) && !raw.contains(AFTER_CUTOFF));
    let diagnostic = trace_contents(env.home.as_path(), "s_http_diagnostic");
    assert!(!diagnostic.contains(DETAIL) && !diagnostic.contains(RESPONSE_REQUEST_ID));
    assert!(diagnostic.contains("invalid_request"));
}
