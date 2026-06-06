mod common;

use std::time::Duration;

use common::{anthropic_sse_response, openai_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use httpmock::When;
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
    assert_eq!(events.len(), 7);
    assert!(matches!(
        events[3].payload,
        EventPayload::ContextPrelude { .. }
    ));
    assert!(matches!(
        events[4].payload,
        EventPayload::ContextSources { .. }
    ));
    assert!(matches!(
        events[5].payload,
        EventPayload::ModelResponse { .. }
    ));
    assert!(matches!(events[6].payload, EventPayload::TurnEnd { .. }));
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
async fn second_turn_request_places_drift_notice_between_context_and_tool_guidance() {
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
                Regex::new(r#"(?s).*<kuku_runtime_context>.*<kuku_system_notice>.*"#).unwrap(),
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
    assert_eq!(events.len(), 7);
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
        .any(|event| matches!(event.payload, EventPayload::TurnEnd { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn missing_config_fails_before_writing_session_events() {
    let env = TestEnv::new();
    let sid = "s_no_cfg";

    let err = query("test").session(sid).run().await.unwrap_err();
    assert!(matches!(err, Error::MissingProviderConfig(_)));

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    assert!(events.is_empty());
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
