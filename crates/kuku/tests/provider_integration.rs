mod common;

use std::time::Duration;

use common::{anthropic_sse_response, openai_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use httpmock::When;
use kuku::agent::registry::AgentRegistry;
use kuku::event::{EventPayload, EventStore};
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
        .body_contains("<kuku_global_memory>")
        .body_contains("<kuku_tool_guidance>")
        .body_contains(query_text)
        .matches(is_initial_request)
}

/// Shorthand for the common Anthropic query builder.
fn anthro(query_text: &str, server: &MockServer) -> query::Query {
    query(query_text)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
}

fn anthro_with_agents(query_text: &str, server: &MockServer) -> query::Query {
    anthro(query_text, server).agents(AgentRegistry::builder().builtins().build())
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

// ---------------------------------------------------------------------------
// simple success — no tools
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn anthropic_success_returns_text_and_writes_events() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .header("x-api-key", "test-key")
            .header("anthropic-version", "2023-06-01");
        then.status(200)
            .header("request-id", "req_abc")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Hello from Claude!"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 10}
            })));
    });

    let output = anthro("say hello", &server).run().await.unwrap();

    mock.assert();
    assert_eq!(output.text, "Hello from Claude!");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert_eq!(events.len(), 9);
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSources { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSkills { .. })));
    assert!(matches!(
        events[events.len() - 2].payload,
        EventPayload::ModelResponse { .. }
    ));
    assert!(matches!(
        events[events.len() - 1].payload,
        EventPayload::TurnCompleted { .. }
    ));
}

// ---------------------------------------------------------------------------
// tool loop — auto-execute (no permission gate)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn executes_find_files_and_continues_to_final_response() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "# Project").unwrap();
    std::fs::create_dir_all(env.workspace.path().join("src")).unwrap();
    std::fs::write(env.workspace.path().join("src/main.rs"), "fn main() {}").unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "find files")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will inspect files."},
                    {"type": "tool_use", "id": "toolu_01", "name": "find_files", "input": {"path": "."}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let catch_all = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "I found README.md and src/main.rs."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("find files", &server).run().await.unwrap();

    tool_mock.assert();
    catch_all.assert();
    assert_eq!(output.text, "I found README.md and src/main.rs.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.payload, EventPayload::ContextSources { .. }))
            .count(),
        2
    );
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelResponse {
            input_tokens_total: Some(_),
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "find_files" && tool_call_id == "toolu_01"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, ref model_content, .. }
            if status == "ok" && model_content.contains("README.md") && model_content.contains("src/main.rs")
    )));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e.payload,
                EventPayload::PermissionAllow { .. } | EventPayload::PermissionDeny { .. }
            ))
            .count(),
        0
    );
}

// ---------------------------------------------------------------------------
// drift detection
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn second_turn_request_wraps_drift_notice_inside_runtime_context() {
    let env = TestEnv::new();
    let first_server = MockServer::start();
    std::fs::write(env.workspace.path().join("AGENTS.md"), "version one").unwrap();

    first_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("version one")
            .body_contains("bootstrap turn");
        then.status(200)
            .header("request-id", "req_first")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_first",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "bootstrap ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let first = query("bootstrap turn")
        .session("s_provider_drift")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(first_server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "bootstrap ok");

    std::fs::write(env.workspace.path().join("AGENTS.md"), "version two").unwrap();

    let second_server = MockServer::start();
    let second_request = second_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_matches(
                Regex::new(
                    r#"(?s).*<kuku_runtime_context>.*<kuku_system_notice>.*</kuku_system_notice>.*</kuku_runtime_context>.*"#,
                )
                .unwrap(),
            )
            .body_contains("Only unacknowledged drift is reported here.")
            .body_contains("- AGENTS.md (updated)");
        then.status(200)
            .header("request-id", "req_second")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_second",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "drift ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let second = query("next turn")
        .session("s_provider_drift")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(second_server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(second.text, "drift ok");

    second_request.assert_hits(1);
}

#[tokio::test(flavor = "current_thread")]
async fn agent_directory_notice_lists_open_conversations() {
    let env = TestEnv::new();
    let session_id = "s_notice_open_conversations";

    let path = env.events_path(session_id);
    let mut store = EventStore::open(&path).unwrap();
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
            conversation: "review".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
            text: "review secret transcript".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent".into()),
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
        })
        .unwrap();

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_open_conversation_summary_without_peer_transcript)
            .body_contains("Available contacts:")
            .body_contains("routing hint:")
            .body_contains("open conversations: 1")
            .body_contains("main followup");
        then.status(200)
            .header("request-id", "req_notice_main")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_notice_main",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main notice ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 6, "output_tokens": 4}
            })));
    });

    let output = anthro_with_agents("main followup", &server)
        .session(session_id)
        .run()
        .await
        .unwrap();
    assert_eq!(output.text, "main notice ok");
}

#[tokio::test(flavor = "current_thread")]
async fn agent_conversation_sees_own_notices_and_incoming_messages_only() {
    let env = TestEnv::new();
    let session_id = "s_notice_review_only";

    let path = env.events_path(session_id);
    let mut store = EventStore::open(&path).unwrap();
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
            conversation: "review".into(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
            text: "please review this".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent_review".into()),
        })
        .unwrap();
    store
        .append(EventPayload::ContextSkills {
            conversation: "review".into(),
            turn: 1,
            ts: "t1".into(),
            registry: serde_json::json!({}),
            bootstrap_loaded: vec!["review-skill".into()],
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            ts: "t2".into(),
            conversation: "review".into(),
            turn: 2,
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 2,
            ts: "t2".into(),
            conversation: Some("review".into()),
            tool_call_id: "toolu_cmd_review".into(),
            request_id: "req_review_2".into(),
            index: 0,
            tool: "run_command".into(),
            args: serde_json::json!({"command": "cargo test"}),
        })
        .unwrap();
    store
        .append(EventPayload::PermissionRequested {
            turn: 2,
            ts: "t2".into(),
            tool_call_id: "toolu_cmd_review".into(),
            tool: "run_command".into(),
            risk: "command".into(),
            summary: "run gated command".into(),
            candidate: "cargo test".into(),
            source: "default_ask".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnInterrupted {
            ts: "t2".into(),
            conversation: "review".into(),
            turn: 2,
            reason: "host_cancelled".into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t3".into(),
            conversation: "explore".into(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t3".into(),
            conversation: "explore".into(),
            turn: 1,
            text: "explore secret transcript".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent_explore".into()),
        })
        .unwrap();

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_review_conversation_notices_only)
            .body_contains("review followup");
        then.status(200)
            .header("request-id", "req_notice_review")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_notice_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review notice ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 6, "output_tokens": 4}
            })));
    });

    let output = anthro_with_agents("review followup", &server)
        .session(session_id)
        .conversation("review")
        .run()
        .await
        .unwrap();
    assert_eq!(output.text, "review notice ok");
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_snapshot_is_conversation_scoped() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_main_not_review);
        then.status(200)
            .header("request-id", "req_main")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let first = query("main snapshot")
        .session("s_snapshot_scope")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "main ok");

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_review_not_main);
        then.status(200)
            .header("request-id", "req_review")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let second = query("review snapshot")
        .session("s_snapshot_scope")
        .conversation("review")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(second.text, "review ok");
}

#[tokio::test(flavor = "current_thread")]
async fn non_main_provider_request_replays_previous_assistant_reply() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    let first_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("first review request")
            .matches(|req| !body_contains(req, b"review followup"));
        then.status(200)
            .header("request-id", "req_review_first")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_first",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "previous review answer"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let first = anthro("first review request", &server)
        .session("s_review_assistant_history")
        .conversation("review")
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "previous review answer");
    first_mock.assert();

    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_review_assistant_history);
        then.status(200)
            .header("request-id", "req_review_second")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_second",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review followup ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let second = anthro("review followup", &server)
        .session("s_review_assistant_history")
        .conversation("review")
        .run()
        .await
        .unwrap();

    assert_eq!(second.text, "review followup ok");
    second_mock.assert();
}

#[tokio::test(flavor = "current_thread")]
async fn provider_request_uses_snapshot_then_history_then_current_input_frame() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    let bootstrap = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_first_input_not_live_input);
        then.status(200)
            .header("request-id", "req_bootstrap")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_bootstrap",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "assistant history reply"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let first = query("first input")
        .session("s_snapshot_order")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "assistant history reply");

    let ordered = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(snapshot_history_and_input_are_in_order);
        then.status(200)
            .header("request-id", "req_ordered")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_ordered",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ordered ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let second = query("live input")
        .session("s_snapshot_order")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(second.text, "ordered ok");

    bootstrap.assert_hits(1);
    ordered.assert_hits(1);
}

#[tokio::test(flavor = "current_thread")]
async fn agent_to_reuses_conversation_address() {
    let env = TestEnv::new();
    let session_id = "s_agent_reuse";

    let server1 = MockServer::start();
    let main_tool_1 = server1.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_initial_request)
            .body_contains("delegate first");
        then.status(200)
            .header("request-id", "req_main_tool_1")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_tool_1",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Delegating first review."},
                    {"type": "tool_use", "id": "toolu_agent_1", "name": "agent", "input": {"to": "review", "message": "review work one"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let followup_1 = server1.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_followup_1")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_followup_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review one done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let mut first = anthro_with_agents("delegate first", &server1)
        .session(session_id)
        .start()
        .await
        .unwrap();
    wait_for_tool_end(&mut first, "toolu_agent_1").await;
    first.cancel();
    main_tool_1.assert_hits(1);
    let _ = followup_1.hits();
    drop(first);

    let server2 = MockServer::start();
    let main_tool_2 = server2.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("delegate second");
        then.status(200)
            .header("request-id", "req_main_tool_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_tool_2",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Delegating second review."},
                    {"type": "tool_use", "id": "toolu_agent_2", "name": "agent", "input": {"to": "review", "message": "review work two"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let followup_2 = server2.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_followup_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_followup_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review two done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let mut second = anthro_with_agents("delegate second", &server2)
        .session(session_id)
        .start()
        .await
        .unwrap();
    wait_for_tool_end(&mut second, "toolu_agent_2").await;
    second.cancel();
    main_tool_2.assert_hits(1);
    let _ = followup_2.hits();

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationBound { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        1
    );
    let review_messages: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::MessageUser {
                conversation,
                from,
                via_tool_call_id,
                ..
            } if conversation == "review" => Some((
                from.as_deref().unwrap_or(""),
                via_tool_call_id.as_deref().unwrap_or(""),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        review_messages,
        vec![("main", "toolu_agent_1"), ("main", "toolu_agent_2")]
    );
    assert!(!tree_contains_name(env.home.path(), "subs"));
    assert!(!tree_contains_name(
        env.home.path(),
        "child_s_agent_reuse_0"
    ));
    assert!(!tree_contains_name(
        env.home.path(),
        "child_s_agent_reuse_1"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn agent_to_opens_nested_address_from_root_contact() {
    let env = TestEnv::new();
    let session_id = "s_agent_nested";
    let server = MockServer::start();

    let main_tool = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_initial_request)
            .body_contains("delegate nested");
        then.status(200)
            .header("request-id", "req_main_tool_nested")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_tool_nested",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Delegating nested review."},
                    {"type": "tool_use", "id": "toolu_agent_nested", "name": "agent", "input": {"to": "review/api", "message": "nested review"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let nested_followup = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_nested_followup")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_nested_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "nested review done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let mut output = anthro_with_agents("delegate nested", &server)
        .session(session_id)
        .start()
        .await
        .unwrap();
    wait_for_tool_end(&mut output, "toolu_agent_nested").await;
    output.cancel();
    main_tool.assert_hits(1);
    let _ = nested_followup.hits();

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    let nested_kinds: Vec<&str> = events
        .iter()
        .filter_map(|event| {
            (event_conversation(&event.payload) == Some("review/api"))
                .then(|| event.payload.kind_name())
        })
        .collect();
    let opened_index = nested_kinds
        .iter()
        .position(|kind| *kind == "conversation.opened")
        .unwrap();
    let bound_index = nested_kinds
        .iter()
        .position(|kind| *kind == "conversation.bound")
        .unwrap();
    let started_index = nested_kinds
        .iter()
        .position(|kind| *kind == "turn.started")
        .unwrap();
    let user_index = nested_kinds
        .iter()
        .position(|kind| *kind == "message.user")
        .unwrap();
    assert!(opened_index < bound_index);
    assert!(bound_index < started_index);
    assert!(started_index < user_index);
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageUser { conversation, from, via_tool_call_id, .. }
            if conversation == "review/api"
                && from.as_deref() == Some("main")
                && via_tool_call_id.as_deref() == Some("toolu_agent_nested")
    )));
    assert!(!tree_contains_name(env.home.path(), "subs"));
    assert!(!tree_contains_name(
        env.home.path(),
        "child_s_agent_nested_0"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn first_turn_request_includes_budgeted_skill_block_and_hints() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let skill_dir = env
        .workspace
        .path()
        .join(".claude")
        .join("skills")
        .join("tdd");
    let mut config = test_config();
    config.discovery.auto_discover = false;
    config.discovery.extra_project_paths = vec![env.workspace.path().join(".claude")];
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: tdd\ndescription: Write tests first\n---\n\nInstructions.\n",
    )
    .unwrap();

    let request = server.mock(|when, then| {
        context_conditions(when, "show skills")
            .method(POST)
            .path("/v1/messages")
            .body_contains("<kuku_skills>")
            .body_contains("Available skills: 1 total")
            .body_contains("tdd - Write tests first")
            .body_contains("Use list_skills to browse available skills.")
            .body_contains("Use search_skills to find skills by task or workflow.");
        then.status(200)
            .header("request-id", "req_skills")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_skills",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "skills ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });
    let output = query("show skills")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
        .run()
        .await
        .unwrap();

    request.assert_hits(1);
    assert_eq!(output.text, "skills ok");
}

#[tokio::test(flavor = "current_thread")]
async fn executes_list_skills_and_continues_to_final_response() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let mut config = test_config();
    config.discovery.auto_discover = false;
    config.discovery.extra_project_paths = vec![env.workspace.path().join(".claude")];
    let skill_dir = env
        .workspace
        .path()
        .join(".claude")
        .join("skills")
        .join("review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review code carefully\n---\n\nReview instructions.\n",
    )
    .unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "browse skills")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_list_skills")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will list skills."},
                    {"type": "tool_use", "id": "toolu_list_skills", "name": "list_skills", "input": {"limit": 5}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Listed skills."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = query("browse skills")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
        .run()
        .await
        .unwrap();

    tool_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Listed skills.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "list_skills" && tool_call_id == "toolu_list_skills"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult {
            ref status,
            ref model_content,
            ref structured,
            ..
        } if status == "ok"
            && model_content.contains("review")
            && structured.as_ref().is_some_and(|value| {
                value["skills"]
                    .as_array()
                    .is_some_and(|skills| skills.iter().any(|skill| skill["name"] == "review"))
            })
    )));
}

// ---------------------------------------------------------------------------
// tool loop — multi-tool auto-execute
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn executes_read_file_and_search_text() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(
        env.workspace.path().join("README.md"),
        "# Project\nTODO root\nDone\n",
    )
    .unwrap();
    std::fs::create_dir_all(env.workspace.path().join("docs")).unwrap();
    std::fs::write(env.workspace.path().join("docs/tools.md"), "TODO docs\n").unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "read and search")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read and search."},
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md", "limit": 2}},
                    {"type": "tool_use", "id": "toolu_search", "name": "search_text", "input": {"pattern": "TODO", "view": "lines"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let catch_all = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Read and search complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read and search", &server).run().await.unwrap();

    tool_mock.assert();
    catch_all.assert();
    assert_eq!(output.text, "Read and search complete.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "read_file" && tool_call_id == "toolu_read"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "search_text" && tool_call_id == "toolu_search"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, ref model_content, ref structured, .. }
            if status == "ok"
                && model_content.contains("1\t# Project")
                && structured.as_ref().is_some_and(|value| value["kind"] == "file_content" && value["read_event_id"].as_u64().is_some())
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, ref model_content, ref structured, .. }
            if status == "ok"
                && model_content.contains("README.md:2: TODO root")
                && structured.as_ref().is_some_and(|value| value["kind"] == "search_results")
    )));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e.payload,
                EventPayload::PermissionAllow { .. } | EventPayload::PermissionDeny { .. }
            ))
            .count(),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn each_slot_read_file_persists_its_own_read_event_id() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "# Project\n").unwrap();
    std::fs::create_dir_all(env.workspace.path().join("docs")).unwrap();
    std::fs::write(env.workspace.path().join("docs/tools.md"), "# Tools\n").unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "read two files")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read both files."},
                    {"type": "tool_use", "id": "toolu_read_1", "name": "read_file", "input": {"path": "README.md"}},
                    {"type": "tool_use", "id": "toolu_read_2", "name": "read_file", "input": {"path": "docs/tools.md"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Reads complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read two files", &server).run().await.unwrap();

    tool_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Reads complete.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    let read_events = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolResult {
                status,
                structured: Some(structured),
                ..
            } if status == "ok" && structured["kind"] == "file_content" => {
                Some((event.id, structured))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(read_events.len(), 2);
    for (event_id, structured) in read_events {
        assert_eq!(structured["read_event_id"], event_id);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn read_file_snapshot_allows_following_edit_file() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "alpha\nbeta\n").unwrap();

    let read_mock = server.mock(|when, then| {
        context_conditions(when, "read then edit")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_read")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_read",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read the file first."},
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let edit_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(has_read_tool_result_only);
        then.status(200)
            .header("request-id", "req_edit")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_edit",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Now I can edit it."},
                    {"type": "tool_use", "id": "toolu_edit", "name": "edit_file", "input": {"path": "README.md", "old_text": "beta", "new_text": "gamma", "brief": "rename beta"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 7, "output_tokens": 8}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(has_edit_tool_result);
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Edit complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read then edit", &server).run().await.unwrap();

    read_mock.assert();
    edit_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Edit complete.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    let read_event = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolResult {
                status,
                structured: Some(structured),
                ..
            } if status == "ok" && structured["kind"] == "file_content" => {
                Some((event.id, structured))
            }
            _ => None,
        })
        .expect("missing read_file tool result");
    assert_eq!(read_event.1["path"], "README.md");
    assert_eq!(read_event.1["read_event_id"], read_event.0);
    assert!(read_event.1["read_event_id"].as_u64().unwrap() > 0);
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { status, structured: Some(structured), .. }
            if status == "ok" && structured["kind"] == "file_edit"
    )));
    assert!(!events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { status, model_content, .. }
            if status == "error"
                && model_content.contains("prior successful read_file snapshot")
    )));
    assert_eq!(
        std::fs::read_to_string(env.workspace.path().join("README.md")).unwrap(),
        "alpha\ngamma\n"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn same_batch_read_file_then_edit_file_succeeds() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "alpha\nbeta\n").unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "read then edit same batch")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read and then edit the file."},
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md"}},
                    {"type": "tool_use", "id": "toolu_edit", "name": "edit_file", "input": {"path": "README.md", "old_text": "beta", "new_text": "gamma", "brief": "rename beta"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(has_read_and_edit_tool_results);
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Same-batch edit complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read then edit same batch", &server)
        .run()
        .await
        .unwrap();

    tool_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Same-batch edit complete.");
    assert_eq!(
        std::fs::read_to_string(env.workspace.path().join("README.md")).unwrap(),
        "alpha\ngamma\n"
    );

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { status, structured: Some(structured), .. }
            if status == "ok" && structured["kind"] == "file_edit"
    )));
    assert!(!events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { status, model_content, .. }
            if status == "error"
                && model_content.contains("prior successful read_file snapshot")
    )));
}

// ---------------------------------------------------------------------------
// permission — allow (streaming)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn can_allow_run_command_once_via_run_decide() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "run tests")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test --version", "timeout": 60, "brief": "check test version"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let catch_all = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let mut run = anthro("run tests", &server).start().await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let event = next_matching(&mut run, deadline, |e| {
        matches!(e, UiEvent::PermissionRequested { .. })
    })
    .await;
    let request = match event {
        UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    };

    run.decide(&request.id, kuku::query::PermissionChoice::Session, None)
        .await
        .unwrap();

    let event = next_matching(&mut run, deadline, |e| matches!(e, UiEvent::Done { .. })).await;
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "Command completed."),
        _ => unreachable!(),
    }

    tool_mock.assert();
    catch_all.assert();

    let events = EventStore::replay(env.events_path(run.session_id())).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionAllow { ref scope, .. }
            if scope == "session"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, .. } if status == "ok"
    )));
}

// ---------------------------------------------------------------------------
// permission — project scope persistence
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn project_scope_allow_persists_to_policy_file_and_applies_on_next_run() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let tool_mock_1 = server.mock(|when, then| {
        context_conditions(when, "run tests")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let catch_all_1 = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final_1")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "First command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let mut run = anthro("run tests", &server).start().await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let event = next_matching(&mut run, deadline, |e| {
        matches!(e, UiEvent::PermissionRequested { .. })
    })
    .await;
    let request = match event {
        UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    };
    run.decide(&request.id, kuku::query::PermissionChoice::Project, None)
        .await
        .unwrap();
    let event = next_matching(&mut run, deadline, |e| matches!(e, UiEvent::Done { .. })).await;
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "First command completed."),
        _ => unreachable!(),
    }

    tool_mock_1.assert();
    catch_all_1.assert();

    // Policy file persisted on disk.
    let policy_path = kuku::session::project_policy_path(
        env.home.path(),
        &std::fs::canonicalize(env.workspace.path()).unwrap(),
    )
    .unwrap();
    let policy_text = std::fs::read_to_string(&policy_path).unwrap();
    assert!(policy_text.contains("run_command(cargo test)"));

    // Second run — permission auto-allowed from persisted policy.
    let server2 = MockServer::start();

    let tool_mock_2 = server2.mock(|when, then| {
        context_conditions(when, "run tests")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool_2",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command again."},
                    {"type": "tool_use", "id": "toolu_cmd_2", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let catch_all_2 = server2.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Second command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("run tests", &server2).run().await.unwrap();

    tool_mock_2.assert();
    catch_all_2.assert();
    assert_eq!(output.text, "Second command completed.");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionAllow { ref scope, .. }
            if scope == "project"
    )));
}

// ---------------------------------------------------------------------------
// permission — denied (auto-deny when no prior grant)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn records_denied_run_command_and_continues() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "run tests")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let catch_all = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Command was blocked."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("run tests", &server).run().await.unwrap();

    tool_mock.assert();
    catch_all.assert();
    assert_eq!(output.text, "Command was blocked.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "run_command" && tool_call_id == "toolu_cmd"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionDeny { ref tool_call_id, ref tool, .. }
            if tool_call_id == "toolu_cmd" && tool == "run_command"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, ref model_content, .. }
            if status == "blocked"
                && model_content.contains("run_command was not executed because the permission gate denied this tool call")
    )));
}

// ---------------------------------------------------------------------------
// openai
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn openai_success_returns_text_and_writes_events() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/chat/completions")
            .header("authorization", "Bearer openai-key");
        then.status(200)
            .body(openai_sse_response(serde_json::json!({
                "choices": [{"message": {"content": "Hi from GPT!"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 4}
            })));
    });

    let output = query("hi")
        .provider(Provider::OpenAiCompatible)
        .model("gpt-5.4-mini")
        .base_url(server.base_url())
        .api_key("openai-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    mock.assert();
    assert_eq!(output.text, "Hi from GPT!");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert_eq!(events.len(), 9);
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSkills { .. })));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelResponse { ref text, .. } if text == "Hi from GPT!"
    )));
}

// ---------------------------------------------------------------------------
// error handling
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn http_error_writes_model_error_and_turn_end() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let sid = "s_http_err";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(401)
            .header("request-id", "req_http_error")
            .body("unauthorized");
    });

    let err = query("test")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("bad")
        .session(sid)
        .config(test_config())
        .run()
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Provider { .. }));

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::TurnInterrupted { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn missing_config_fails_before_writing_session_events() {
    let env = TestEnv::new();
    let sid = "s_no_cfg";

    let err = query("test").session(sid).run().await.unwrap_err();
    assert!(matches!(err, Error::MissingProviderConfig(_)));

    let events_path = env.events_path(sid);
    if events_path.exists() {
        let events = EventStore::replay(&events_path).unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::ModelError { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::TurnInterrupted { .. })));
    }
}

// ---------------------------------------------------------------------------
// security
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn api_key_is_not_written_to_events() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let sid = "s_no_leak";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })));
    });

    query("test")
        .provider(Provider::Anthropic)
        .model("m")
        .api_key("secret-123")
        .base_url(server.base_url())
        .session(sid)
        .config(test_config())
        .run()
        .await
        .unwrap();

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    let raw = format!("{events:?}");
    assert!(!raw.contains("secret-123"));
}
