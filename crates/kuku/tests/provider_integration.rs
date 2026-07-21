mod common;

use std::time::Duration;

use common::{anthropic_sse_response, openai_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use httpmock::When;
use kuku::agent::registry::AgentRegistry;
use kuku::config::{SecretString, StoredCredential};
use kuku::event::{EventPayload, EventStore};
use kuku::prompt::builtin_prompt_catalog;
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

fn body_contains(req: &HttpMockRequest, needle: &[u8]) -> bool {
    req.body
        .as_ref()
        .is_some_and(|body| body.windows(needle.len()).any(|window| window == needle))
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

fn body_contains_main_not_review(req: &HttpMockRequest) -> bool {
    body_contains(req, b"main snapshot") && !body_contains(req, b"review snapshot")
}

fn body_contains_review_not_main(req: &HttpMockRequest) -> bool {
    body_contains(req, b"review snapshot") && !body_contains(req, b"main snapshot")
}

fn body_contains_review_assistant_history(req: &HttpMockRequest) -> bool {
    body_contains(req, b"review followup") && body_contains(req, b"previous review answer")
}

fn body_contains_child_agent_result(req: &HttpMockRequest) -> bool {
    body_contains(req, b"\"tool_use_id\":\"toolu_agent_child\"")
        && body_contains(req, b"KUKU_CHILD_AGENT_RESULT")
}

fn body_contains_initial_child_task(req: &HttpMockRequest) -> bool {
    body_contains(req, b"child task") && is_initial_request(req)
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

fn body_contains_open_conversation_summary_without_peer_transcript(req: &HttpMockRequest) -> bool {
    body_contains(req, b"Open conversations:")
        && body_contains(req, b"review: turn 1 completed")
        && !body_contains(req, b"review secret transcript")
}

fn body_contains_review_conversation_notices_only(req: &HttpMockRequest) -> bool {
    body_contains(req, b"review followup")
        && body_contains(req, b"please review this")
        && !body_contains(req, b"explore secret transcript")
}

/// Register the common body conditions that always accompany a tool-use
/// request.  Returns the updated `When` for further chaining.
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
    provider.credential = StoredCredential::DirectValue(SecretString::new("test-key"));

    query(query_text)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
}

fn anthro_with_agents(query_text: &str, server: &MockServer) -> query::Query {
    anthro(query_text, server).agents(
        AgentRegistry::builder()
            .builtins(&builtin_prompt_catalog())
            .build(),
    )
}

fn event_conversation(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::ConversationOpened { conversation, .. }
        | EventPayload::ConversationBound { conversation, .. }
        | EventPayload::MessageUser { conversation, .. }
        | EventPayload::MessageAssistant { conversation, .. }
        | EventPayload::TurnStarted { conversation, .. }
        | EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. } => Some(conversation.as_str()),
        _ => None,
    }
}

fn tree_contains_name(root: &std::path::Path, needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == needle) {
            return true;
        }
        if path.is_dir() && tree_contains_name(&path, needle) {
            return true;
        }
    }
    false
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

async fn wait_for_tool_end(run: &mut Run, tool_call_id: &str) -> UiEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    next_matching(run, deadline, |event| {
        matches!(
            event,
            UiEvent::ToolEnd {
                id,
                status: _,
                summary: _,
                model_content: _,
                result: _,
            } if id == tool_call_id
        )
    })
    .await
}

include!("provider_integration/core.rs");
include!("provider_integration/agents_and_tools.rs");
include!("provider_integration/permissions_and_errors.rs");
