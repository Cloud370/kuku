mod common;

use common::{anthropic_sse_response, openai_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use kuku::event::{EventPayload, EventStore};
use kuku::{query, Error, Provider};
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

    let output = query("say hello")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    mock.assert();
    assert_eq!(output.text, "Hello from Claude!");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert_eq!(events.len(), 6);
    assert!(matches!(
        events[3].payload,
        EventPayload::ModelRequest { .. }
    ));
    assert!(matches!(
        events[4].payload,
        EventPayload::ModelResponse { .. }
    ));
    assert!(matches!(events[5].payload, EventPayload::TurnEnd { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_tool_loop_executes_find_files_and_continues_to_final_response() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "# Project").unwrap();
    std::fs::create_dir_all(env.workspace.path().join("src")).unwrap();
    std::fs::write(env.workspace.path().join("src/main.rs"), "fn main() {}").unwrap();

    let final_request = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result"#)
            .body_contains("README.md")
            .body_contains("src/main.rs");
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
    let tool_request = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tools"#)
            .body_contains("<kuku_execution_context>")
            .body_contains("Workspace root:")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("find files");
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

    let output = query("find files")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    tool_request.assert();
    final_request.assert();
    assert_eq!(output.text, "I found README.md and src/main.rs.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelRequest {
            tool_count: Some(8),
            ref ordered_tool_names,
            ref tool_registry_hash,
            ..
        } if ordered_tool_names.as_ref().is_some_and(|names| names[0] == "find_files")
            && ordered_tool_names.as_ref().is_some_and(|names| names.contains(&"memory.remember".to_string()))
            && ordered_tool_names.as_ref().is_some_and(|names| names.contains(&"memory.forget".to_string()))
            && tool_registry_hash.as_ref().is_some_and(|hash| hash.starts_with("sha256:"))
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelResponse {
            tool_call_count: Some(1),
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
            .filter(|event| matches!(event.payload, EventPayload::ModelRequest { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::PermissionRequest { .. }))
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::PermissionDecision { .. }))
            .count(),
        0
    );
}

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
            .body_matches(Regex::new(r#"(?s).*<kuku_execution_context>.*<kuku_system_notice>.*<kuku_tool_guidance>.*"#).unwrap())
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
async fn anthropic_tool_loop_executes_read_file_and_search_text() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(
        env.workspace.path().join("README.md"),
        "# Project\nTODO root\nDone\n",
    )
    .unwrap();
    std::fs::create_dir_all(env.workspace.path().join("docs")).unwrap();
    std::fs::write(env.workspace.path().join("docs/tools.md"), "TODO docs\n").unwrap();

    let final_request = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result"#)
            .body_contains("1\\t# Project")
            .body_contains("README.md:2: TODO root")
            .body_contains("docs/tools.md:1: TODO docs");
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
    let tool_request = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tools"#)
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("read and search");
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

    let output = query("read and search")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    tool_request.assert();
    final_request.assert();
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
            .filter(|event| matches!(event.payload, EventPayload::PermissionRequest { .. }))
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::PermissionDecision { .. }))
            .count(),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_tool_loop_can_allow_run_command_once_via_run_decide() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("cargo test");
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
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tools""#)
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("run tests");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test --version", "timeout": 60}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let mut run = query("run tests")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let mut event = run.next().await.unwrap().unwrap();
    while !matches!(event, kuku::UiEvent::PermissionRequested { .. }) {
        event = run.next().await.unwrap().unwrap();
    }
    let request = match event {
        kuku::UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    };

    run.decide(&request.id, kuku::query::PermissionChoice::Once)
        .await
        .unwrap();

    let mut event = run.next().await.unwrap().unwrap();
    while !matches!(event, kuku::UiEvent::Done { .. }) {
        event = run.next().await.unwrap().unwrap();
    }
    match event {
        kuku::UiEvent::Done { output, .. } => assert_eq!(output.text, "Command completed."),
        _ => unreachable!(),
    }

    let events = EventStore::replay(env.events_path(run.session_id())).unwrap();
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionDecision { ref decision, ref scope, .. } if decision == "allow" && scope == "once")));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "ok")));
}

#[tokio::test(flavor = "current_thread")]
async fn project_scope_allow_persists_to_policy_file_and_applies_on_next_run() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#);
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
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tools""#)
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("run tests");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let mut run = query("run tests")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let mut event = run.next().await.unwrap().unwrap();
    while !matches!(event, kuku::UiEvent::PermissionRequested { .. }) {
        event = run.next().await.unwrap().unwrap();
    }
    let request = match event {
        kuku::UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    };
    run.decide(&request.id, kuku::query::PermissionChoice::Project)
        .await
        .unwrap();
    let mut event = run.next().await.unwrap().unwrap();
    while !matches!(event, kuku::UiEvent::Done { .. }) {
        event = run.next().await.unwrap().unwrap();
    }
    match event {
        kuku::UiEvent::Done { output, .. } => assert_eq!(output.text, "First command completed."),
        _ => unreachable!(),
    }

    let policy_path = kuku::session::project_policy_path(
        env.home.path(),
        &std::fs::canonicalize(env.workspace.path()).unwrap(),
    )
    .unwrap();
    let policy_text = std::fs::read_to_string(&policy_path).unwrap();
    assert!(policy_text.contains("run_command(cargo test)"));

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#);
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
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tools""#)
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("run tests");
        then.status(200)
            .header("request-id", "req_tool_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool_2",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command again."},
                    {"type": "tool_use", "id": "toolu_cmd_2", "name": "run_command", "input": {"command": "cargo test", "timeout": 60}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let output = query("run tests")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    assert_eq!(output.text, "Second command completed.");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionDecision { ref decision, ref scope, .. } if decision == "allow" && scope == "project")));
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_tool_loop_records_denied_run_command_and_continues() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let final_request = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result"#)
            .body_contains(
                "run_command was not executed because the permission gate denied this tool call",
            );
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
    let tool_request = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(r#""tools"#)
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("run tests");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let output = query("run tests")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    tool_request.assert();
    final_request.assert();
    assert_eq!(output.text, "Command was blocked.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "run_command" && tool_call_id == "toolu_cmd"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionRequest { ref tool, ref risk, .. }
            if tool == "run_command" && risk == "command"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionDecision { ref tool_call_id, ref decision, .. }
            if tool_call_id == "toolu_cmd" && decision == "deny"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, ref model_content, .. }
            if status == "blocked"
                && model_content.contains("run_command was not executed because the permission gate denied this tool call")
    )));
}

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
    assert_eq!(events.len(), 6);
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelResponse { ref stop_reason, .. } if stop_reason == "end_turn"
    )));
}

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

    assert!(matches!(err, Error::Provider(_)));

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

    // Config error happens before any session events are written.
    let events = EventStore::replay(env.events_path(sid)).unwrap();
    assert!(events.is_empty());
}

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
