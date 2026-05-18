mod common;

use common::{anthropic_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use kuku::event::{EventPayload, EventStore};
use kuku::{query, Error, Provider, UiEvent};
#[tokio::test(flavor = "current_thread")]
async fn start_creates_session_events_under_kuku_home() {
    let env = TestEnv::new();

    let run = query("inspect this project")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let session_id = run.session_id().to_string();

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].id, 1);
    assert_eq!(events[1].id, 2);
    assert_eq!(events[2].id, 3);

    match &events[0].payload {
        EventPayload::SessionMeta {
            schema_version,
            session_id: meta_session_id,
            kuku_version,
            ts,
            created_at,
        } => {
            assert_eq!(*schema_version, 1);
            assert_eq!(meta_session_id, &session_id);
            assert_eq!(kuku_version, env!("CARGO_PKG_VERSION"));
            assert!(ts.ends_with('Z'));
            assert!(created_at.ends_with('Z'));
        }
        other => panic!("expected session.meta, got {other:?}"),
    }

    match &events[1].payload {
        EventPayload::TurnStart { turn, ts } => {
            assert_eq!(*turn, 1);
            assert!(ts.ends_with('Z'));
        }
        other => panic!("expected turn.start, got {other:?}"),
    }

    match &events[2].payload {
        EventPayload::UserInput { turn, text, ts } => {
            assert_eq!(*turn, 1);
            assert_eq!(text, "inspect this project");
            assert!(ts.ends_with('Z'));
        }
        other => panic!("expected user.input, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_without_config_fails_before_writing_events() {
    let env = TestEnv::new();

    let error = query("summarize")
        .session("s_run_fixed")
        .run()
        .await
        .unwrap_err();

    assert!(matches!(error, Error::MissingProviderConfig(_)));
    // Config error happens before any session events are written.
    let events = EventStore::replay(env.events_path("s_run_fixed")).unwrap();
    assert!(events.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn provider_step_uses_captured_kuku_home_for_memory_sources() {
    let env = TestEnv::new();
    std::fs::write(env.home.path().join("memory.md"), "captured-session-memory").unwrap();

    let runtime_home = tempfile::tempdir().unwrap();
    std::fs::write(runtime_home.path().join("memory.md"), "runtime-memory").unwrap();

    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("captured-session-memory");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final_memory",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Captured home memory."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });

    let mut run = query("summarize memory")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    std::env::set_var("KUKU_HOME", runtime_home.path());

    let mut event = run.next().await.unwrap().expect("event");
    while !matches!(event, UiEvent::Done { .. }) {
        event = run.next().await.unwrap().expect("event");
    }
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "Captured home memory."),
        _ => unreachable!(),
    }

    mock.assert();
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_session_start_appends_turn_without_duplicate_meta() {
    let env = TestEnv::new();

    query("first")
        .session("s_continue")
        .config(test_config())
        .start()
        .await
        .unwrap();
    query("second")
        .session("s_continue")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let events = EventStore::replay(env.events_path("s_continue")).unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::SessionMeta { .. }))
            .count(),
        1
    );

    assert!(matches!(
        events[1].payload,
        EventPayload::TurnStart { turn: 1, .. }
    ));
    assert!(matches!(
        events[2].payload,
        EventPayload::UserInput { turn: 1, .. }
    ));
    assert!(matches!(
        events[3].payload,
        EventPayload::TurnStart { turn: 2, .. }
    ));
    match &events[4].payload {
        EventPayload::UserInput { turn, text, .. } => {
            assert_eq!(*turn, 2);
            assert_eq!(text, "second");
        }
        other => panic!("expected second user.input, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_is_not_polluted() {
    let env = TestEnv::new();

    let _ = query("no pollution")
        .config(test_config())
        .run()
        .await
        .unwrap_err();

    assert_eq!(std::fs::read_dir(env.workspace_path()).unwrap().count(), 0);
    assert!(!env.workspace_path().join(".kuku").exists());
    assert!(!env.workspace_path().join(".kuku-id").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_session_ids_fail_before_creating_session_path() {
    let env = TestEnv::new();

    for session_id in [
        "../bad",
        "CON",
        "con",
        "COM1",
        "LPT9",
        "CON.txt",
        "aux.log",
        "LPT1.json",
        "name.",
        "name ",
    ] {
        let error = query("bad")
            .session(session_id)
            .config(test_config())
            .run()
            .await
            .unwrap_err();
        assert!(matches!(error, Error::InvalidSessionId(ref value) if value == session_id));
    }

    assert!(!env.home.path().join("p").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn run_emits_permission_requested_for_gated_tool() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
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

    let mut event = run.next().await.unwrap().expect("event");
    while !matches!(event, UiEvent::PermissionRequested { .. }) {
        event = run.next().await.unwrap().expect("event");
    }
    match event {
        UiEvent::PermissionRequested { request } => {
            assert_eq!(request.tool_call_id, "toolu_cmd");
            assert_eq!(request.tool, "run_command");
        }
        _ => unreachable!(),
    }

    let events = EventStore::replay(env.events_path(run.session_id())).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::PermissionRequest { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::PermissionDecision { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn session_scope_allow_is_reused_on_later_turn_in_same_session() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "First command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("run tests");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_session_grant";
    let mut run = query("run tests")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let mut event = run.next().await.unwrap().expect("event");
    while !matches!(event, UiEvent::PermissionRequested { .. }) {
        event = run.next().await.unwrap().expect("event");
    }
    let request = match event {
        UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    };
    run.decide(&request.id, kuku::query::PermissionChoice::Session)
        .await
        .unwrap();
    let mut event = run.next().await.unwrap().expect("event");
    while !matches!(event, UiEvent::Done { .. }) {
        event = run.next().await.unwrap().expect("event");
    }
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "First command completed."),
        _ => unreachable!(),
    }

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Second command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("run tests");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool_2",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval again."},
                    {"type": "tool_use", "id": "toolu_cmd_2", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let mut run = query("run tests")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let mut event = run.next().await.unwrap().expect("event");
    while !matches!(event, UiEvent::Done { .. }) {
        event = run.next().await.unwrap().expect("event");
    }
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "Second command completed."),
        _ => unreachable!(),
    }

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionDecision { ref decision, ref scope, .. } if decision == "allow" && scope == "session")));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "ok")));
}

#[tokio::test(flavor = "current_thread")]
async fn run_convenience_path_auto_denies_and_continues_when_approval_is_needed() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("permission gate denied this tool call");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Command was blocked."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("<kuku_execution_context>")
            .body_contains("<kuku_project_instructions>")
            .body_contains("<kuku_memory>")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("run tests");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
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

    assert_eq!(output.text, "Command was blocked.");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionDecision { ref decision, ref scope, .. } if decision == "deny" && scope == "once")));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "blocked")));
}

#[tokio::test(flavor = "current_thread")]
async fn new_top_level_turn_can_surface_context_drift_notice_for_changed_tracked_files() {
    let env = TestEnv::new();
    let server = MockServer::start();

    std::fs::write(env.workspace.path().join("AGENTS.md"), "version one").unwrap();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("version one")
            .body_contains("first turn");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_first",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "first ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let first = query("first turn")
        .session("s_drift_notice")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "first ok");

    std::fs::write(env.workspace.path().join("AGENTS.md"), "version two").unwrap();

    let second_server = MockServer::start();
    second_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("<kuku_system_notice>")
            .body_contains("Only unacknowledged drift is reported here.")
            .body_contains("This notice does not include the changed file contents.")
            .body_contains("Changed tracked files:")
            .body_contains("- AGENTS.md (updated)")
            .body_contains("second turn");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_second",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "second ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let second = query("second turn")
        .session("s_drift_notice")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(second_server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(second.text, "second ok");
}

#[tokio::test(flavor = "current_thread")]
async fn new_top_level_turn_can_surface_deleted_tracked_files_in_context_drift_notice() {
    let env = TestEnv::new();
    let server = MockServer::start();

    std::fs::write(env.workspace.path().join("AGENTS.md"), "version one").unwrap();
    std::fs::write(env.workspace.path().join("notes.md"), "hello\n").unwrap();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("1\\thello");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_done",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "first ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("first turn")
            .body_contains(r#""tools""#)
            .body_contains("version one");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_first",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read the file."},
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "notes.md"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let first = query("first turn")
        .session("s_drift_deleted_notice")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "first ok");

    std::fs::remove_file(env.workspace.path().join("notes.md")).unwrap();

    let second_server = MockServer::start();
    second_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("<kuku_system_notice>")
            .body_contains("Changed tracked files:")
            .body_contains("- notes.md (deleted)")
            .body_contains("second turn");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_second",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "second ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let second = query("second turn")
        .session("s_drift_deleted_notice")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(second_server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(second.text, "second ok");
}

#[tokio::test(flavor = "current_thread")]
async fn model_request_persists_prompt_assets_and_loaded_source_hashes() {
    let env = TestEnv::new();
    let server = MockServer::start();

    std::fs::write(
        env.workspace.path().join("AGENTS.md"),
        "follow repo instructions",
    )
    .unwrap();
    std::fs::write(env.home.path().join("memory.md"), "global memory entry").unwrap();

    let workspace = std::fs::canonicalize(env.workspace.path()).unwrap();
    let project_home = kuku::session::project_home(env.home.path(), &workspace).unwrap();
    std::fs::create_dir_all(&project_home).unwrap();
    std::fs::write(project_home.join("memory.md"), "project memory entry").unwrap();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("<kuku_execution_context>")
            .body_contains("Current date:")
            .body_contains("<kuku_project_instructions>")
            .body_contains("follow repo instructions")
            .body_contains("<kuku_memory>")
            .body_contains("global memory entry")
            .body_contains("project memory entry")
            .body_contains("<kuku_tool_guidance>")
            .body_contains("<kuku_working_style>");
        then.status(200).json_body(serde_json::json!({
            "id": "msg_final",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        }));
    });

    let output = query("say ok")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    let model_request = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ModelRequest {
                provenance: Some(provenance),
                ..
            } => Some(provenance.clone()),
            _ => None,
        })
        .expect("model.request provenance");

    assert_eq!(
        model_request["prompt_asset_sources"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        model_request["project_instruction_sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(model_request["memory_sources"].as_array().unwrap().len(), 2);
    assert_eq!(
        model_request["platform"].as_str().unwrap(),
        match std::env::consts::OS {
            "linux" => "linux",
            "windows" => "windows",
            "macos" => "macos",
            _ => "unknown",
        }
    );
    assert_eq!(
        model_request["current_date"].as_str().unwrap(),
        time::OffsetDateTime::now_utc().date().to_string()
    );
    assert!(model_request["tool_registry"]["hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    let prompt_assets = model_request["prompt_asset_sources"].as_array().unwrap();
    assert!(prompt_assets
        .iter()
        .any(|entry| entry["path"] == "crates/kuku/prompts/system.md"));
    assert!(prompt_assets
        .iter()
        .any(|entry| entry["path"] == "crates/kuku/prompts/project-context.md"));
    assert!(prompt_assets
        .iter()
        .any(|entry| entry["path"] == "crates/kuku/prompts/tool-guidance.md"));
}
