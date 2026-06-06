mod common;

use common::{anthropic_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use kuku::event::{EventPayload, EventStore};
use kuku::log::{LogLevel, LogRecord, LogScope};
use kuku::{query, Error, PermissionChoice, PermissionRequest, Provider, Run, UiEvent};

async fn next_permission_request(run: &mut Run) -> PermissionRequest {
    let mut event = run.next().await.unwrap().expect("event");
    while !matches!(event, UiEvent::PermissionRequested { .. }) {
        event = run.next().await.unwrap().expect("event");
    }
    match event {
        UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    }
}
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
async fn start_persists_session_scoped_log_without_event_payload() {
    let env = TestEnv::new();

    let run = query("inspect this project")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let session_id = run.session_id().to_string();

    let session_log_path = env
        .home
        .path()
        .join("logs")
        .join("session")
        .join(format!("{session_id}.jsonl"));
    let content = std::fs::read_to_string(session_log_path).unwrap();
    let records: Vec<LogRecord> = content
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert!(records.iter().any(|record| {
        record.kind == "session.turn_start"
            && record.scope == LogScope::Session
            && record.session_id.as_deref() == Some(session_id.as_str())
            && record.run_id.as_deref() == Some(session_id.as_str())
            && record.turn == Some(1)
    }));

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_eq!(events.len(), 3);
    assert!(!events.iter().any(|event| {
        let payload = serde_json::to_value(&event.payload).unwrap();
        payload.get("log").is_some() || payload.get("debug").is_some()
    }));
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
async fn runtime_logs_are_fanned_out_without_events_payloads_or_immediate_info_flush() {
    let env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200).body(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"stream overloaded\"}}\n\n",
        );
    });

    let mut run = query("emit diagnostics")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let session_id = run.session_id().to_string();
    let log_dir = env.home.path().join("logs").join("runtime");
    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::TurnStart { turn: 1 })
    ));
    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::ModelRequest { .. })
    ));
    let log = run.next().await.unwrap().expect("runtime log");
    match log {
        UiEvent::Log { record } => {
            assert_eq!(record.kind, "runtime.model_request");
            assert_eq!(record.level, LogLevel::Info);
            assert_eq!(record.scope, LogScope::Runtime);
            assert_eq!(record.session_id.as_deref(), Some(session_id.as_str()));
            assert_eq!(record.run_id.as_deref(), Some(session_id.as_str()));
            assert!(
                !log_dir.exists() || std::fs::read_dir(&log_dir).unwrap().next().is_none(),
                "info log should be host-visible before disk flush"
            );
        }
        other => panic!("expected runtime log, got {other:?}"),
    }

    let error = run.next().await.unwrap_err();
    assert!(matches!(error, Error::Provider { .. }));

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_failed_turn_facts(&events, 1);
    assert!(!events.iter().any(|event| {
        let payload = serde_json::to_value(&event.payload).unwrap();
        payload.get("log").is_some() || payload.get("debug").is_some()
    }));

    let records: Vec<kuku::log::LogRecord> = std::fs::read_dir(&log_dir)
        .unwrap()
        .flat_map(|entry| {
            let content = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            content
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(records
        .iter()
        .any(|record| record.kind == "runtime.model_request"
            && record.session_id.as_deref() == Some(session_id.as_str())));
}

#[tokio::test(flavor = "current_thread")]
async fn run_convenience_path_persists_buffered_runtime_info_logs_on_completion() {
    let env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_log_run",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Run response."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });

    let output = query("persist diagnostics")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    let log_dir = env.home.path().join("logs").join("runtime");
    let records: Vec<kuku::log::LogRecord> = std::fs::read_dir(&log_dir)
        .unwrap()
        .flat_map(|entry| {
            let content = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            content
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();

    assert_eq!(output.text, "Run response.");
    assert!(records
        .iter()
        .any(|record| record.kind == "runtime.model_request"
            && record.session_id.as_deref() == Some(output.session_id.as_str())));
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_log_preserves_turn_model_log_event_order() {
    let _env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_log_order",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Ordered response."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });

    let mut run = query("ordered diagnostics")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let first = run.next().await.unwrap().expect("turn start");
    let second = run.next().await.unwrap().expect("model request");
    let third = run.next().await.unwrap().expect("log");

    assert!(matches!(first, UiEvent::TurnStart { turn: 1 }));
    assert!(matches!(second, UiEvent::ModelRequest { .. }));
    assert!(matches!(third, UiEvent::Log { ref record } if record.kind == "runtime.model_request"));
}

#[tokio::test(flavor = "current_thread")]
async fn provider_start_failure_still_delivers_runtime_model_request_log() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let mut run = query("provider fails")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let session_id = run.session_id().to_string();
    let log_dir = env.home.path().join("logs").join("runtime");

    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::TurnStart { turn: 1 })
    ));
    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::ModelRequest { .. })
    ));
    let log = run.next().await.unwrap().expect("runtime log");
    assert!(matches!(log, UiEvent::Log { ref record } if record.kind == "runtime.model_request"));

    let error = run.next().await.unwrap_err();
    assert!(matches!(error, Error::Provider { .. }));

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_failed_turn_facts(&events, 1);
    assert!(!events.iter().any(|event| {
        let payload = serde_json::to_value(&event.payload).unwrap();
        payload.get("log").is_some() || payload.get("debug").is_some()
    }));

    let records: Vec<kuku::log::LogRecord> = std::fs::read_dir(&log_dir)
        .unwrap()
        .flat_map(|entry| {
            let content = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            content
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(records
        .iter()
        .any(|record| record.kind == "runtime.model_request"
            && record.session_id.as_deref() == Some(session_id.as_str())));
}

#[tokio::test(flavor = "current_thread")]
async fn context_too_large_failure_still_delivers_runtime_model_request_log() {
    let env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(413).body("context too large");
    });

    let mut run = query("context overflow")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let session_id = run.session_id().to_string();

    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::TurnStart { turn: 1 })
    ));
    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::ModelRequest { .. })
    ));
    let log = run.next().await.unwrap().expect("runtime log");
    assert!(matches!(log, UiEvent::Log { ref record } if record.kind == "runtime.model_request"));

    let error = run.next().await.unwrap_err();
    assert!(matches!(error, Error::Provider { .. }));

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_failed_turn_facts(&events, 1);
    assert!(!events.iter().any(|event| {
        let payload = serde_json::to_value(&event.payload).unwrap();
        payload.get("log").is_some() || payload.get("debug").is_some()
    }));
}

fn assert_failed_turn_facts(events: &[kuku::event::StoredEvent], turn: u64) {
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelError { turn: event_turn, .. } if event_turn == turn
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::TurnEnd { turn: event_turn, .. } if event_turn == turn
    )));
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
    let tool_call_pos = events
        .iter()
        .position(|event| {
            matches!(event.payload, EventPayload::ToolCall { ref tool_call_id, .. } if tool_call_id == "toolu_cmd")
        })
        .expect("tool.call event");
    let permission_pos = events
        .iter()
        .position(|event| {
            matches!(event.payload, EventPayload::PermissionRequested { ref tool_call_id, .. } if tool_call_id == "toolu_cmd")
        })
        .expect("permission.requested event");

    assert!(tool_call_pos < permission_pos);
    match &events[permission_pos].payload {
        EventPayload::PermissionRequested {
            turn,
            tool_call_id,
            tool,
            risk,
            summary,
            candidate,
            source,
            ..
        } => {
            assert_eq!(*turn, 1);
            assert_eq!(tool_call_id, "toolu_cmd");
            assert_eq!(tool, "run_command");
            assert_eq!(risk, "command");
            assert_eq!(summary, "run tests");
            assert_eq!(candidate, "cargo test");
            assert_eq!(source, "default_ask");
        }
        other => panic!("expected permission.requested, got {other:?}"),
    }
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionAllow { .. } | EventPayload::PermissionDeny { .. }
    )));
    assert!(!events.iter().any(|event| {
        let payload = serde_json::to_value(&event.payload).unwrap();
        payload.get("log").is_some() || payload.get("debug").is_some()
    }));
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
            .body_contains("<kuku_global_memory>")
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
    run.decide(&request.id, kuku::query::PermissionChoice::Session, None)
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
    drop(run);

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
            .body_contains("<kuku_global_memory>")
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
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionAllow { ref scope, .. } if scope == "session")));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "ok")));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_reemits_request_before_new_turn() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_permission",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_resume_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_permission_request";
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

    let original = next_permission_request(&mut run).await;
    drop(run);

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    match resumed.next().await.unwrap().expect("resumed event") {
        UiEvent::PermissionRequested { request } => {
            assert_eq!(request.id, original.id);
            assert_eq!(request.tool_call_id, original.tool_call_id);
            assert_eq!(request.tool, original.tool);
            assert_eq!(request.candidate, original.candidate);
            assert_eq!(request.source, original.source);
        }
        other => panic!("expected resumed permission request, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_does_not_append_new_turn_before_decision() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_no_turn",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_no_turn_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_no_new_turn";
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

    let _request = next_permission_request(&mut run).await;
    drop(run);

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    assert!(matches!(
        resumed.next().await.unwrap(),
        Some(UiEvent::PermissionRequested { .. })
    ));

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    let turn_starts = events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::TurnStart { .. }))
        .count();
    let user_inputs: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::UserInput { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(turn_starts, 1);
    assert_eq!(user_inputs, vec!["run tests"]);
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_does_not_resolve_config_or_append_facts_before_reemit() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_no_resolve",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_no_resolve", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_no_resolve";
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

    let original = next_permission_request(&mut run).await;
    let before_events = EventStore::replay(env.events_path(session_id)).unwrap();
    drop(run);

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    match resumed.next().await.unwrap().expect("resumed request") {
        UiEvent::PermissionRequested { request } => assert_eq!(request.id, original.id),
        other => panic!("expected resumed permission request, got {other:?}"),
    }

    let after_events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert_eq!(after_events.len(), before_events.len());
    assert!(!after_events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelError { .. } | EventPayload::TurnEnd { .. }
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_decide_continues_without_duplicate_turn_or_request_id() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("permission gate denied this tool call");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Command denied after resume."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_decide",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_resume_decide", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_decide_continues";
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

    let request = next_permission_request(&mut run).await;
    drop(run);

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    assert!(matches!(
        resumed.next().await.unwrap(),
        Some(UiEvent::PermissionRequested { .. })
    ));
    let decision_event = resumed
        .decide(&request.id, PermissionChoice::Deny, None)
        .await
        .unwrap();
    assert!(matches!(decision_event, Some(UiEvent::ToolEnd { .. })));

    let mut saw_resumed_turn_start = false;
    let mut event = resumed.next().await.unwrap().expect("event after decision");
    while !matches!(event, UiEvent::Done { .. }) {
        if matches!(event, UiEvent::TurnStart { turn: 1 }) {
            saw_resumed_turn_start = true;
        }
        event = resumed.next().await.unwrap().expect("event after decision");
    }

    assert!(!saw_resumed_turn_start);

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    let request_ids: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ModelResponse { request_id, .. } => Some(request_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(request_ids, vec!["req_1", "req_2"]);
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_allow_executes_original_tool() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let final_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains(r#""tool_use_id":"toolu_resume_allow""#)
            .body_contains("resumed-allowed");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_allow_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Allowed after resume."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("run resumed command");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_allow_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_resume_allow", "name": "run_command", "input": {"command": "printf resumed-allowed", "timeout": 60, "brief": "print resumed marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_allow_executes";
    let mut run = query("run resumed command")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let request = next_permission_request(&mut run).await;
    drop(run);

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    assert!(matches!(
        resumed.next().await.unwrap(),
        Some(UiEvent::PermissionRequested { .. })
    ));
    let decision_event = resumed
        .decide(&request.id, PermissionChoice::Once, None)
        .await
        .unwrap();
    assert!(
        matches!(decision_event, Some(UiEvent::ToolStart { id, .. }) if id == "toolu_resume_allow")
    );

    let mut event = resumed.next().await.unwrap().expect("event after allow");
    while !matches!(event, UiEvent::Done { .. }) {
        event = resumed.next().await.unwrap().expect("event after allow");
    }
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "Allowed after resume."),
        _ => unreachable!(),
    }

    final_mock.assert_hits(1);
    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionAllow { ref tool_call_id, .. } if tool_call_id == "toolu_resume_allow"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref tool_call_id, ref status, .. }
            if tool_call_id == "toolu_resume_allow" && status == "ok"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_deny_records_real_deny() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let final_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("permission gate denied this tool call");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_deny_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Denied after resume."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("deny resumed command");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_deny_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_resume_deny", "name": "run_command", "input": {"command": "printf should-not-run", "timeout": 60, "brief": "print denied marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_deny_records";
    let mut run = query("deny resumed command")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let request = next_permission_request(&mut run).await;
    drop(run);

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    assert!(matches!(
        resumed.next().await.unwrap(),
        Some(UiEvent::PermissionRequested { .. })
    ));
    let decision_event = resumed
        .decide(&request.id, PermissionChoice::Deny, None)
        .await
        .unwrap();
    assert!(matches!(decision_event, Some(UiEvent::ToolEnd { status, .. }) if status == "blocked"));

    let mut event = resumed.next().await.unwrap().expect("event after deny");
    while !matches!(event, UiEvent::Done { .. }) {
        event = resumed.next().await.unwrap().expect("event after deny");
    }

    final_mock.assert_hits(1);
    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionDeny { ref tool_call_id, ref source, .. }
            if tool_call_id == "toolu_resume_deny" && source == "host"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref tool_call_id, ref status, ref model_content, .. }
            if tool_call_id == "toolu_resume_deny" && status == "blocked" && model_content.contains("permission gate denied")
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_cancel_writes_cancelled_result_without_deny() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("cancel resumed command");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_cancel_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_resume_cancel", "name": "run_command", "input": {"command": "printf should-not-run", "timeout": 60, "brief": "print cancelled marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_cancel_records";
    let mut run = query("cancel resumed command")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let request = next_permission_request(&mut run).await;
    drop(run);

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    assert!(matches!(
        resumed.next().await.unwrap(),
        Some(UiEvent::PermissionRequested { .. })
    ));
    let event = resumed.cancel_pending_permission(&request.id).unwrap();
    assert!(
        matches!(event, Some(UiEvent::ToolEnd { status, result, .. }) if status == "cancelled" && result == Some(serde_json::json!({"kind": "cancelled"})))
    );

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionDeny { ref tool_call_id, .. } if tool_call_id == "toolu_resume_cancel"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref tool_call_id, ref status, ref structured, .. }
            if tool_call_id == "toolu_resume_cancel" && status == "cancelled" && structured == &Some(serde_json::json!({"kind": "cancelled"}))
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_preserves_sibling_queued_permission() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_resume_siblings",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need two approvals."},
                    {"type": "tool_use", "id": "toolu_resume_first", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}},
                    {"type": "tool_use", "id": "toolu_resume_second", "name": "run_command", "input": {"command": "cargo check", "timeout": 60, "brief": "run check"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_sibling_permission";
    let mut run = query("run tests then check")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let first = next_permission_request(&mut run).await;
    assert_eq!(first.tool_call_id, "toolu_resume_first");

    let mut store = EventStore::open(_env.events_path(session_id)).unwrap();
    store
        .append(EventPayload::PermissionRequested {
            turn: 1,
            ts: "2026-06-06T00:00:00Z".to_string(),
            tool_call_id: "toolu_resume_second".to_string(),
            tool: "run_command".to_string(),
            risk: "command".to_string(),
            summary: "persisted second summary".to_string(),
            candidate: "persisted cargo check".to_string(),
            source: "persisted_source".to_string(),
        })
        .unwrap();
    let before_events = EventStore::replay(_env.events_path(session_id)).unwrap();
    drop(run);

    let mut invalid_config = test_config();
    invalid_config.default_tier = "missing-tier".to_string();

    let mut resumed = query("second prompt must not be appended yet")
        .session(session_id)
        .config(invalid_config)
        .start()
        .await
        .unwrap();

    let resumed_first = match resumed.next().await.unwrap().expect("resumed request") {
        UiEvent::PermissionRequested { request } => request,
        other => panic!("expected first resumed permission, got {other:?}"),
    };
    assert_eq!(resumed_first.tool_call_id, "toolu_resume_first");

    let denied = resumed
        .decide(&resumed_first.id, PermissionChoice::Deny, None)
        .await
        .unwrap();
    assert!(matches!(denied, Some(UiEvent::ToolEnd { .. })));

    let resumed_second = match resumed.next().await.unwrap().expect("second request") {
        UiEvent::PermissionRequested { request } => request,
        other => panic!("expected resumed sibling permission, got {other:?}"),
    };
    assert_eq!(resumed_second.tool_call_id, "toolu_resume_second");
    assert_eq!(resumed_second.tool, "run_command");
    assert_eq!(resumed_second.candidate, "persisted cargo check");
    assert_eq!(resumed_second.source, "persisted_source");

    let events = EventStore::replay(_env.events_path(session_id)).unwrap();
    assert_eq!(events.len(), before_events.len() + 2);
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelError { .. } | EventPayload::TurnEnd { .. }
    )));
    let requested_second = events
        .iter()
        .filter(|event| {
            matches!(event.payload, EventPayload::PermissionRequested { ref tool_call_id, .. } if tool_call_id == "toolu_resume_second")
        })
        .count();
    assert_eq!(requested_second, 1);
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
            .body_contains("<kuku_global_memory>")
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
    let requested = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::PermissionRequested {
                tool_call_id,
                tool,
                risk,
                candidate,
                source,
                ..
            } if tool_call_id == "toolu_cmd" => Some((
                tool.as_str(),
                risk.as_str(),
                candidate.as_str(),
                source.as_str(),
            )),
            _ => None,
        })
        .expect("permission.requested event");
    assert_eq!(
        requested,
        ("run_command", "command", "cargo test", "default_ask")
    );
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::PermissionDeny { .. })));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "blocked")));
}

#[tokio::test(flavor = "current_thread")]
async fn queued_deny_path_emits_permission_requested_before_deny() {
    let env = TestEnv::new();
    std::fs::write(env.workspace.path().join("notes.md"), "hello\n").unwrap();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("read two files");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tools",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Reading files."},
                    {"type": "tool_use", "id": "toolu_read_ok", "name": "read_file", "input": {"path": "notes.md"}},
                    {"type": "tool_use", "id": "toolu_read_denied", "name": "read_file", "input": {"path": ".env.local"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let mut run = query("read two files")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let mut saw_first_start = false;
    loop {
        let event = run.next().await.unwrap().expect("event");
        match event {
            UiEvent::ToolStart { id, .. } if id == "toolu_read_ok" => saw_first_start = true,
            UiEvent::ToolEnd { id, status, .. } if id == "toolu_read_denied" => {
                assert!(
                    saw_first_start,
                    "first slot should be active before queued deny"
                );
                assert_eq!(status, "blocked");
                break;
            }
            _ => {}
        }
    }

    let events = EventStore::replay(env.events_path(run.session_id())).unwrap();
    let request_pos = events
        .iter()
        .position(|event| {
            matches!(event.payload, EventPayload::PermissionRequested { ref tool_call_id, .. } if tool_call_id == "toolu_read_denied")
        })
        .expect("permission.requested event");
    let deny_pos = events
        .iter()
        .position(|event| {
            matches!(event.payload, EventPayload::PermissionDeny { ref tool_call_id, .. } if tool_call_id == "toolu_read_denied")
        })
        .expect("permission.deny event");

    assert!(request_pos < deny_pos);
    match &events[request_pos].payload {
        EventPayload::PermissionRequested {
            tool,
            risk,
            candidate,
            source,
            ..
        } => {
            assert_eq!(tool, "read_file");
            assert_eq!(risk, "read");
            assert_eq!(candidate, ".env.local");
            assert_eq!(source, "hard_guard");
        }
        other => panic!("expected permission.requested, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_permission_choice_allows_gated_tool_and_continues() {
    let env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Command was allowed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("run allowed command");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "printf allowed", "timeout": 60, "brief": "print marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let output = query("run allowed command")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run_with_permission_choice(PermissionChoice::Once)
        .await
        .unwrap();

    assert_eq!(output.text, "Command was allowed.");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::PermissionAllow { ref scope, .. } if scope == "once")));
    assert!(events.iter().any(|event| matches!(event.payload, EventPayload::ToolResult { ref status, .. } if status == "ok")));
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
    let specific = second_server.mock(|when, then| {
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
    assert_eq!(specific.hits(), 1);
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
            .body_contains("<kuku_global_memory>")
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
    let context_sources = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ContextSources {
                project_instruction_sources,
                memory_sources,
                ..
            } => Some((project_instruction_sources.clone(), memory_sources.clone())),
            _ => None,
        })
        .expect("context.sources fact event");

    assert_eq!(context_sources.0.len(), 1);
    assert_eq!(context_sources.1.len(), 2);
    assert!(context_sources
        .0
        .iter()
        .any(|entry| entry.path.ends_with("AGENTS.md")));
    assert!(context_sources
        .1
        .iter()
        .any(|entry| entry.path.ends_with("memory.md")));
}
