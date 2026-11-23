mod common;

use std::time::Duration;

use common::{anthropic_sse_response, openai_sse_response, test_config, TestEnv};

use httpmock::prelude::{HttpMockRequest, MockServer, Regex, POST};
use httpmock::When;
use kuku::config::{ApiKey, ProviderProfile};
use kuku::event::{EventPayload, EventStore, ModelStopReason};
use kuku::query::Run;
use kuku::{query, Error, Provider, UiEvent};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Returns `true` when the request is the *first* API call of a turn — the
/// one that carries the user's query and has no tool results yet.
///
/// It locates the final `"role":"user"}` message that ends the `"messages"`
/// array (using `rposition`) and checks whether that message contains a
/// `"tool_use_id"` field, which only appears in `tool_result` content blocks
/// sent back after tool execution.  A function pointer is required because
/// httpmock 0.7 `matches()` accepts `fn(&HttpMockRequest) -> bool`, not
/// closures with captures.
// Byte-window lengths for JSON-pattern matching in request bodies.
const TOOL_USE_LEN: usize = b"\"type\":\"tool_use\"".len(); // 17
const TOOL_RESULT_LEN: usize = b"\"type\":\"tool_result\"".len(); // 20

/// Returns `true` when the request body has neither a `tool_use` nor a
/// `tool_result` content block — i.e. it is the very first API call of a turn.
/// The second call already carries the assistant's `"type":"tool_use"` block
/// (before the tool has executed), and the third carries `"type":"tool_result"`.
fn is_initial_request(req: &HttpMockRequest) -> bool {
    let Some(body) = req.body.as_ref() else {
        return false;
    };
    let has_tool_use = body
        .windows(TOOL_USE_LEN)
        .any(|w| w == b"\"type\":\"tool_use\"");
    let has_tool_result = body
        .windows(TOOL_RESULT_LEN)
        .any(|w| w == b"\"type\":\"tool_result\"");
    !has_tool_use && !has_tool_result
}

fn is_original_request(req: &HttpMockRequest) -> bool {
    is_initial_request(req)
        && !body_contains(req, b"previous model attempt reached its output limit")
}

fn is_original_frozen_notice_request(req: &HttpMockRequest) -> bool {
    is_initial_request(req) && !body_contains(req, b"FROZEN_RECOVERY_NOTICE_V1")
}

fn body_contains(req: &HttpMockRequest, needle: &[u8]) -> bool {
    req.body
        .as_ref()
        .is_some_and(|body| body.windows(needle.len()).any(|window| window == needle))
}

fn responses_sse(id: &str, text: &str, terminal: &str) -> String {
    format!(
        "event: response.created\n\
         data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"{id}\",\"status\":\"in_progress\"}}}}\n\n\
         event: response.output_text.delta\n\
         data: {{\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":{text}}}\n\n\
         {terminal}",
        text = serde_json::to_string(text).unwrap(),
    )
}

fn body_contains_first_input_not_live_input(req: &HttpMockRequest) -> bool {
    body_contains(req, b"first input") && !body_contains(req, b"live input")
}

fn snapshot_history_and_input_are_in_order(req: &HttpMockRequest) -> bool {
    let Some(body) = req.body.as_ref() else {
        return false;
    };

    if !body_contains(req, b"live input") {
        return false;
    }

    let history = br#"assistant history reply"#;
    let frame = br#"<input.message>live input</input.message>"#;

    let locate = |needle: &[u8]| {
        body.windows(needle.len())
            .position(|window| window == needle)
    };

    match (locate(history), locate(frame)) {
        (Some(history_pos), Some(frame_pos)) => history_pos < frame_pos,
        _ => true,
    }
}

fn has_read_tool_result_only(req: &HttpMockRequest) -> bool {
    body_contains(req, b"\"tool_use_id\":\"toolu_read\"")
        && !body_contains(req, b"\"tool_use_id\":\"toolu_edit\"")
}

fn has_edit_tool_result(req: &HttpMockRequest) -> bool {
    body_contains(req, b"\"tool_use_id\":\"toolu_edit\"")
}

fn has_read_and_edit_tool_results(req: &HttpMockRequest) -> bool {
    body_contains(req, b"\"tool_use_id\":\"toolu_read\"")
        && body_contains(req, b"\"tool_use_id\":\"toolu_edit\"")
}

fn context_conditions(when: When, query_text: &str) -> When {
    when.body_contains(r#""tools""#)
        .body_contains("<kuku_execution_context>")
        .body_contains("<kuku_project_instructions>")
        .body_contains("<kuku_tool_guidance>")
        .body_contains(query_text)
        .matches(is_initial_request)
}

/// Shorthand for the common Anthropic query builder.
fn anthro(query_text: &str, server: &MockServer) -> query::Query {
    let mut config = test_config();
    let light_tier = config
        .tiers
        .get("balanced")
        .expect("test config has balanced tier")
        .clone();
    config.tiers.insert("light".to_string(), light_tier);
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
}

fn anthro_profile(
    query_text: &str,
    server: &MockServer,
    base_path: &str,
    profile: ProviderProfile,
) -> query::Query {
    let mut config = test_config();
    let provider = config
        .providers
        .get_mut("anthropic")
        .expect("test config has anthropic provider");
    provider.options.profile = profile;
    provider.base_url = server.url(base_path);
    provider.api_key = ApiKey::Plaintext("test-key".to_string());

    query(query_text)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.url(base_path))
        .api_key("test-key")
        .config(config)
}

async fn next_matching(
    run: &mut Run,
    deadline: tokio::time::Instant,
    pred: impl Fn(&UiEvent) -> bool,
) -> UiEvent {
    loop {
        let remaining = deadline.duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, run.next()).await {
            Ok(Ok(Some(event))) if pred(&event) => return event,
            Ok(Ok(Some(_))) => continue,
            Ok(Ok(None)) => panic!("stream ended before matching event"),
            Ok(Err(e)) => panic!("run error: {e}"),
            Err(_) => panic!("timed out waiting for matching UiEvent"),
        }
    }
}

include!("provider_integration/protocols.rs");
include!("provider_integration/context.rs");
include!("provider_integration/tools.rs");
