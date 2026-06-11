mod common;

use common::{anthropic_sse_response, test_config, TestEnv};

use httpmock::prelude::*;
use kuku::agent::registry::AgentRegistry;
use kuku::context::rebuild_history;
use kuku::conversation::address::ConversationAddress;
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

async fn next_tool_end(run: &mut Run, tool_call_id: &str) -> UiEvent {
    loop {
        let event = run.next().await.unwrap().expect("event");
        if matches!(&event, UiEvent::ToolEnd { id, .. } if id == tool_call_id) {
            return event;
        }
    }
}

fn anthro_with_agents(query_text: &str, server: &MockServer) -> kuku::query::Query {
    query(query_text)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .agents(AgentRegistry::builder().builtins().build())
}

fn request_body_contains(req: &HttpMockRequest, text: &str) -> bool {
    req.body.as_ref().is_some_and(|body| {
        body.windows(text.len())
            .any(|window| window == text.as_bytes())
    })
}

fn request_body_text(req: &HttpMockRequest) -> String {
    String::from_utf8_lossy(req.body.as_deref().unwrap_or_default()).into_owned()
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
    assert_eq!(events.len(), 5);
    assert_eq!(events[0].id, 1);
    assert_eq!(events[1].id, 2);
    assert_eq!(events[2].id, 3);
    assert_eq!(events[3].id, 4);
    assert_eq!(events[4].id, 5);

    match &events[0].payload {
        EventPayload::SessionCreated {
            schema_version,
            session_id: meta_session_id,
            kuku_version,
            ts,
            created_at,
        } => {
            assert_eq!(*schema_version, 2);
            assert_eq!(meta_session_id, &session_id);
            assert_eq!(kuku_version, env!("CARGO_PKG_VERSION"));
            assert!(ts.ends_with('Z'));
            assert!(created_at.ends_with('Z'));
        }
        other => panic!("expected session.created, got {other:?}"),
    }

    match &events[1].payload {
        EventPayload::ConversationOpened { conversation, ts } => {
            assert_eq!(conversation, "main");
            assert!(ts.ends_with('Z'));
        }
        other => panic!("expected conversation.opened, got {other:?}"),
    }

    match &events[2].payload {
        EventPayload::TurnStarted {
            conversation,
            turn,
            ts,
        } => {
            assert_eq!(conversation, "main");
            assert_eq!(*turn, 1);
            assert!(ts.ends_with('Z'));
        }
        other => panic!("expected turn.started, got {other:?}"),
    }

    match &events[3].payload {
        EventPayload::MessageUser {
            conversation,
            turn,
            text,
            ts,
            ..
        } => {
            assert_eq!(conversation, "main");
            assert_eq!(*turn, 1);
            assert_eq!(text, "inspect this project");
            assert!(ts.ends_with('Z'));
        }
        other => panic!("expected message.user, got {other:?}"),
    }

    match &events[4].payload {
        EventPayload::ContextSkills {
            turn,
            bootstrap_loaded,
            ..
        } => {
            assert_eq!(*turn, 1);
            assert!(bootstrap_loaded.is_empty());
        }
        other => panic!("expected context.skills, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_rollback_is_scoped() {
    let env = TestEnv::new();
    let session_id = "s_conversation_rollback_scoped";
    let events_path = env.events_path(session_id);
    std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-09T00:00:00Z".to_string(),
            schema_version: 2,
            session_id: session_id.to_string(),
            created_at: "2026-06-09T00:00:00Z".to_string(),
            kuku_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "2026-06-09T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "2026-06-09T00:00:01Z".to_string(),
            conversation: "review".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:02Z".to_string(),
            conversation: "main".to_string(),
            turn: 1,
            text: "main-1".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:03Z".to_string(),
            conversation: "review".to_string(),
            turn: 1,
            text: "review-1".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:04Z".to_string(),
            conversation: "review".to_string(),
            turn: 2,
            text: "review-2".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    let rollback = store
        .append(EventPayload::ConversationRollback {
            ts: "2026-06-09T00:00:05Z".to_string(),
            conversation: "review".to_string(),
            to_turn: 1,
            to_event_id: 5,
            scope: kuku::event::RollbackScope::ConversationOnly,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:06Z".to_string(),
            conversation: "main".to_string(),
            turn: 2,
            text: "main-2".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ConversationRollbackUndone {
            ts: "2026-06-09T00:00:07Z".to_string(),
            conversation: "review".to_string(),
            rollback_event_id: rollback.id,
        })
        .unwrap();
    let events = EventStore::replay(&events_path).unwrap();
    let review = ConversationAddress::parse("review").unwrap();
    let (_, review_history) = rebuild_history(&events, &review);
    let (_, main_history) = rebuild_history(&events, &ConversationAddress::MAIN);
    let review_texts: Vec<&str> = review_history
        .iter()
        .filter_map(|message| match message.blocks.first() {
            Some(kuku::context::MessageBlock::Text(text)) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let main_texts: Vec<&str> = main_history
        .iter()
        .filter_map(|message| match message.blocks.first() {
            Some(kuku::context::MessageBlock::Text(text)) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(review_texts, vec!["review-1", "review-2"]);
    assert_eq!(main_texts, vec!["main-1", "main-2"]);
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ConversationRollback { .. })));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ConversationRollbackUndone { .. }
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn main_conversation_startup_writes_conversation_events() {
    let env = TestEnv::new();

    let run = query("inspect this project")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let events = EventStore::replay(env.events_path(run.session_id())).unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event.payload.kind_name())
        .collect();

    assert_eq!(
        kinds,
        vec![
            "session.created",
            "conversation.opened",
            "turn.started",
            "message.user",
            "context.skills",
        ]
    );
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
    assert_eq!(events.len(), 5);
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
async fn builder_only_provider_config_starts_without_file_config() {
    let env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_builder_only",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Builder config works."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });

    let mut run = query("builder only")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .session("s_builder_only")
        .start()
        .await
        .unwrap();

    let mut saw_done = false;
    while let Some(event) = run.next().await.unwrap() {
        if let UiEvent::Done { output, .. } = event {
            assert_eq!(output.text, "Builder config works.");
            saw_done = true;
            break;
        }
    }

    assert!(saw_done, "expected done event");
    let events = EventStore::replay(env.events_path("s_builder_only")).unwrap();
    assert!(!events.is_empty());
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
        &event.payload,
        EventPayload::ModelError { turn: event_turn, .. } if *event_turn == turn
    )));
    assert_single_terminal_kind(events, turn, "turn.interrupted");
}

fn assert_single_terminal_kind(
    events: &[kuku::event::StoredEvent],
    turn: u64,
    expected_kind: &str,
) {
    let terminal_events: Vec<&kuku::event::StoredEvent> = events
        .iter()
        .filter(|event| match &event.payload {
            EventPayload::TurnCompleted {
                conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnCancelled {
                conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnInterrupted {
                conversation,
                turn: event_turn,
                ..
            } => *event_turn == turn && conversation == "main",
            _ => false,
        })
        .collect();

    assert_eq!(
        terminal_events.len(),
        1,
        "expected exactly one terminal event"
    );
    assert_eq!(terminal_events[0].payload.kind_name(), expected_kind);
}

#[tokio::test(flavor = "current_thread")]
async fn truncated_provider_stream_is_recorded_as_failed_turn() {
    let env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200).body(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_truncated\",\"type\":\"message\",\"role\":\"assistant\",\"usage\":{\"input_tokens\":7}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        );
    });

    let mut run = query("trigger truncation")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let session_id = run.session_id().to_string();

    loop {
        match run.next().await {
            Ok(Some(UiEvent::TextDelta { .. }))
            | Ok(Some(UiEvent::TurnStart { .. }))
            | Ok(Some(UiEvent::ModelRequest { .. }))
            | Ok(Some(UiEvent::Log { .. })) => continue,
            Ok(Some(other)) => panic!("unexpected event: {other:?}"),
            Ok(None) => panic!("expected provider error"),
            Err(error) => {
                assert!(matches!(
                    error,
                    Error::Provider {
                        kind: kuku::ProviderFailureKind::Transport,
                        ..
                    }
                ));
                break;
            }
        }
    }

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_failed_turn_facts(&events, 1);
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
    assert_eq!(events.len(), 9);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::SessionCreated { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ConversationOpened { ref conversation, .. } if conversation == "main"))
            .count(),
        1
    );

    assert!(matches!(
        events[1].payload,
        EventPayload::ConversationOpened { .. }
    ));
    assert!(matches!(
        events[2].payload,
        EventPayload::TurnStarted { turn: 1, .. }
    ));
    assert!(matches!(
        events[3].payload,
        EventPayload::MessageUser { turn: 1, .. }
    ));
    assert!(matches!(
        events[4].payload,
        EventPayload::ContextSkills { turn: 1, .. }
    ));
    assert!(matches!(
        events[5].payload,
        EventPayload::TurnInterrupted { turn: 1, .. }
    ));
    assert!(matches!(
        events[6].payload,
        EventPayload::TurnStarted { turn: 2, .. }
    ));
    assert!(
        !matches!(events[6].payload, EventPayload::MessageUser { .. }),
        "expected turn.started, got second message.user position"
    );
    assert!(matches!(
        events[7].payload,
        EventPayload::MessageUser { turn: 2, .. }
    ));
    match &events[8].payload {
        EventPayload::ContextSkills {
            turn,
            bootstrap_loaded,
            ..
        } => {
            assert_eq!(*turn, 2);
            assert!(bootstrap_loaded.is_empty());
        }
        other => panic!("expected second context.skills, got {other:?}"),
    }
    match &events[7].payload {
        EventPayload::MessageUser {
            conversation,
            turn,
            text,
            ..
        } => {
            assert_eq!(conversation, "main");
            assert_eq!(*turn, 2);
            assert_eq!(text, "second");
        }
        other => panic!("expected second message.user, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resume_marks_unterminated_main_turn_interrupted() {
    let env = TestEnv::new();
    let session_id = "s_resume_marks_interrupted";
    let events_path = env.events_path(session_id);
    std::fs::create_dir_all(events_path.parent().unwrap()).unwrap();
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-09T00:00:00Z".to_string(),
            schema_version: 2,
            session_id: session_id.to_string(),
            created_at: "2026-06-09T00:00:00Z".to_string(),
            kuku_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "2026-06-09T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            ts: "2026-06-09T00:00:02Z".to_string(),
            conversation: "main".to_string(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "2026-06-09T00:00:03Z".to_string(),
            conversation: "main".to_string(),
            turn: 1,
            text: "first".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    drop(store);

    query("second")
        .session(session_id)
        .config(test_config())
        .start()
        .await
        .unwrap();

    let events = EventStore::replay(&events_path).unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event.payload.kind_name())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "session.created",
            "conversation.opened",
            "turn.started",
            "message.user",
            "turn.interrupted",
            "turn.started",
            "message.user",
            "context.skills",
        ]
    );
    assert!(matches!(
        &events[4].payload,
        EventPayload::TurnInterrupted { conversation, turn: 1, .. } if conversation == "main"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn provider_error_writes_single_terminal_event() {
    let env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(500).body("provider exploded");
    });

    let mut run = query("provider failure")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let session_id = run.session_id().to_string();
    while let Ok(Some(_)) = run.next().await {}

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { turn: 1, .. })));
    assert_single_terminal_kind(&events, 1, "turn.interrupted");
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_render_error_writes_single_terminal_event() {
    let env = TestEnv::new();
    let prompts_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        prompts_dir.path().join("project-context.md"),
        "{{missing_key}}",
    )
    .unwrap();

    let error = query("prompt render failure")
        .config(test_config())
        .prompts_dir(prompts_dir.path())
        .run()
        .await
        .unwrap_err();

    assert!(matches!(error, Error::PromptRender(_)));
    let session_entries = list_event_files(env.home.path());
    assert_eq!(session_entries.len(), 1);
    let events = EventStore::replay(&session_entries[0]).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { turn: 1, .. })));
    assert_single_terminal_kind(&events, 1, "turn.interrupted");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_terminal_writes_for_same_turn_are_ignored() {
    let env = TestEnv::new();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(500).body("provider exploded");
    });

    let mut run = query("provider failure")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let session_id = run.session_id().to_string();
    while let Ok(Some(_)) = run.next().await {}

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_single_terminal_kind(&events, 1, "turn.interrupted");
}

#[tokio::test(flavor = "current_thread")]
async fn skill_attachment_is_conversation_scoped() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let session_id = "s_skill_attachment_scope";
    let mut config = test_config();
    config.discovery.auto_discover = false;
    config.discovery.extra_project_paths = vec![env.workspace.path().join(".claude")];

    let skills_root = env.workspace.path().join(".claude").join("skills");
    let main_skill_dir = skills_root.join("main-skill");
    let api_skill_dir = skills_root.join("api-skill");
    std::fs::create_dir_all(&main_skill_dir).unwrap();
    std::fs::create_dir_all(&api_skill_dir).unwrap();
    std::fs::write(
        main_skill_dir.join("SKILL.md"),
        "---\nname: main-skill\ndescription: Main scoped skill\n---\n\nMain skill instructions.\n",
    )
    .unwrap();
    std::fs::write(
        api_skill_dir.join("SKILL.md"),
        "---\nname: api-skill\ndescription: API scoped skill\n---\n\nAPI skill instructions.\n",
    )
    .unwrap();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "main use skill")
                    && !request_body_contains(req, "Main skill instructions.")
            });
        then.status(200).body(anthropic_sse_response(serde_json::json!({
            "id": "msg_main_tool",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "load main skill"},
                {"type": "tool_use", "id": "toolu_main_skill", "name": "use_skill", "input": {"skill_name": "main-skill"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        })));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "Main skill instructions.")
                && request_body_contains(req, "main use skill")
                && !request_body_contains(req, "main followup")
                && !request_body_contains(req, "review followup")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_done",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main loaded"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });
    let main_loaded = query("main use skill")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(main_loaded.text, "main loaded");

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "review prompt")
                && !request_body_contains(req, "review followup")
                && !request_body_contains(req, "Main skill instructions.")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review clean"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    let review_clean = query("review prompt")
        .session(session_id)
        .conversation("review")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(review_clean.text, "review clean");

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "review api use skill")
                    && !request_body_contains(req, "API skill instructions.")
            });
        then.status(200).body(anthropic_sse_response(serde_json::json!({
            "id": "msg_api_tool",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "load api skill"},
                {"type": "tool_use", "id": "toolu_api_skill", "name": "use_skill", "input": {"skill_name": "api-skill"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        })));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "API skill instructions.")
                && !request_body_contains(req, "Main skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_api_done",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "api loaded"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });
    let api_loaded = query("review api use skill")
        .session(session_id)
        .conversation("review/api")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(api_loaded.text, "api loaded");

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "main followup")
                && request_body_contains(req, "Main skill instructions.")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main still scoped"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    let main_followup = query("main followup")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(main_followup.text, "main still scoped");

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "review followup")
                && !request_body_contains(req, "Main skill instructions.")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review still clean"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    let review_followup = query("review followup")
        .session(session_id)
        .conversation("review")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
        .run()
        .await
        .unwrap();
    assert_eq!(review_followup.text, "review still clean");

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ConversationBound { conversation, .. } if conversation == "main"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ConversationBound { conversation, .. } if conversation == "review/api"
    )));
    assert!(!events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ConversationBound { conversation, .. } if conversation == "review"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn delegated_agent_request_includes_contact_card_instructions() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let mut config = test_config();
    config.tiers.insert(
        "strong".to_string(),
        kuku::config::TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            think: kuku::config::ThinkLevel::Medium,
            context_window: 200_000,
            max_output_tokens: 48_000,
            purpose: "strong".to_string(),
        },
    );
    config.providers.get_mut("anthropic").unwrap().base_url = server.base_url();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "delegate review")
                    && !request_body_contains(req, "<kuku_delegated_prompt>")
                    && !request_body_contains(req, "check </kuku_delegated_prompt> & <tag> > boundary")
            });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "delegating"},
                    {"type": "tool_use", "id": "toolu_review_card", "name": "agent", "input": {"to": "review", "message": "check </kuku_delegated_prompt> & <tag> > boundary"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let child_request = server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            let body = request_body_text(req);
            request_body_contains(req, "You are a code and document reviewer")
                && request_body_contains(req, "Your job is to read the provided context carefully")
                && request_body_contains(req, "<kuku_delegated_prompt>")
                && request_body_contains(
                    req,
                    "check &lt;/kuku_delegated_prompt&gt; &amp; &lt;tag&gt; &gt; boundary",
                )
                && request_body_contains(req, "</kuku_delegated_prompt>")
                && body.matches("</kuku_delegated_prompt>").count() == 1
                && !request_body_contains(req, "check </kuku_delegated_prompt> & <tag> > boundary")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_card",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_review_card\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });

    let mut run = anthro_with_agents("delegate review", &server)
        .session("s_agent_contact_card")
        .config(config.clone())
        .start()
        .await
        .unwrap();
    next_tool_end(&mut run, "toolu_review_card").await;
    run.cancel();
    drop(run);

    child_request.assert();

    let events = EventStore::replay(env.events_path("s_agent_contact_card")).unwrap();
    let child_message = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::MessageUser {
                conversation, text, ..
            } if conversation == "review" => Some(text),
            _ => None,
        })
        .expect("child message.user");
    assert_eq!(
        child_message,
        "check </kuku_delegated_prompt> & <tag> > boundary"
    );
    assert!(!child_message.contains("You are a code and document reviewer"));
    assert!(!child_message.contains("<kuku_delegated_prompt>"));

    let review_snapshot_messages = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::PromptSnapshot {
                conversation,
                messages,
                ..
            } if conversation == "review" => Some(messages),
            _ => None,
        })
        .expect("review prompt.snapshot");
    let review_snapshot_text = review_snapshot_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(review_snapshot_text.contains("You are a code and document reviewer"));
    assert!(!review_snapshot_text.contains("<kuku_delegated_prompt>"));
    assert!(!review_snapshot_text.contains("check </kuku_delegated_prompt> & <tag> > boundary"));

    let second_server = MockServer::start();
    config.providers.get_mut("anthropic").unwrap().base_url = second_server.base_url();
    second_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "delegate review again")
                    && !request_body_contains(req, "<kuku_delegated_prompt>")
            });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_again_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "delegating again"},
                    {"type": "tool_use", "id": "toolu_review_again", "name": "agent", "input": {"to": "review", "message": "second review boundary"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let second_child_request = second_server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "You are a code and document reviewer")
                && request_body_contains(req, "<kuku_delegated_prompt>")
                && request_body_contains(req, "second review boundary")
                && request_body_contains(req, "</kuku_delegated_prompt>")
                && !request_body_contains(req, "check </kuku_delegated_prompt> & <tag> > boundary")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_again",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review again done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    second_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_review_again\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_again_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "done again"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });

    let mut second_run = anthro_with_agents("delegate review again", &second_server)
        .session("s_agent_contact_card")
        .config(config)
        .start()
        .await
        .unwrap();
    next_tool_end(&mut second_run, "toolu_review_again").await;
    second_run.cancel();

    second_child_request.assert();
}

fn list_event_files(kuku_home: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn visit(dir: &std::path::Path, paths: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, paths);
            } else if path.file_name().is_some_and(|name| name == "events.jsonl") {
                paths.push(path);
            }
        }
    }

    let mut paths = Vec::new();
    let root = kuku_home.join("p");
    if root.exists() {
        visit(&root, &mut paths);
    }
    paths
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
async fn agent_tool_rejects_reserved_main_and_tier_conflict() {
    let env = TestEnv::new();

    let reserved_server = MockServer::start();
    reserved_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("reserved main");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_reserved_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "try reserved"},
                    {"type": "tool_use", "id": "toolu_reserved", "name": "agent", "input": {"to": "main", "message": "bad target"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    reserved_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_reserved\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_reserved_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "reserved handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut reserved = anthro_with_agents("reserved main", &reserved_server)
        .session("s_agent_reserved")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut reserved, "toolu_reserved").await;
    reserved.cancel();
    let reserved_events = EventStore::replay(env.events_path("s_agent_reserved")).unwrap();
    assert!(reserved_events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { tool_call_id, status, summary, .. }
            if tool_call_id == "toolu_reserved"
                && status == "error"
                && summary.contains("reserved conversation address 'main'")
    )));
    assert_eq!(
        reserved_events
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationOpened { ref conversation, .. } if conversation == "main"
            ))
            .count(),
        1
    );

    let invalid_server = MockServer::start();
    invalid_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("invalid address");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_invalid_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "try invalid"},
                    {"type": "tool_use", "id": "toolu_invalid", "name": "agent", "input": {"to": "review//api", "message": "bad target"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    invalid_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_invalid\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_invalid_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "invalid handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut invalid = anthro_with_agents("invalid address", &invalid_server)
        .session("s_agent_invalid")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut invalid, "toolu_invalid").await;
    invalid.cancel();
    let invalid_events = EventStore::replay(env.events_path("s_agent_invalid")).unwrap();
    assert!(invalid_events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { tool_call_id, status, summary, .. }
            if tool_call_id == "toolu_invalid"
                && status == "error"
                && summary.contains("invalid slash placement")
    )));
    assert!(!invalid_events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review//api"
    )));

    let unknown_server = MockServer::start();
    unknown_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("unknown contact");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_unknown_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "try unknown"},
                    {"type": "tool_use", "id": "toolu_unknown", "name": "agent", "input": {"to": "unknown", "message": "bad target"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    unknown_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_unknown\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_unknown_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "unknown handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut unknown = anthro_with_agents("unknown contact", &unknown_server)
        .session("s_agent_unknown")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut unknown, "toolu_unknown").await;
    unknown.cancel();
    let unknown_events = EventStore::replay(env.events_path("s_agent_unknown")).unwrap();
    assert!(unknown_events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { tool_call_id, status, summary, .. }
            if tool_call_id == "toolu_unknown"
                && status == "error"
                && summary.contains("unknown agent contact: unknown")
    )));
    assert!(!unknown_events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ConversationOpened { ref conversation, .. } if conversation == "unknown"
    )));

    let establish_server = MockServer::start();
    establish_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("establish review");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_establish_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "establish"},
                    {"type": "tool_use", "id": "toolu_establish", "name": "agent", "input": {"to": "review", "message": "initial review"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    establish_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .matches(|req| {
                req.body.as_ref().is_some_and(|body| {
                    body.windows(b"initial review".len())
                        .any(|w| w == b"initial review")
                })
            });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_establish_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    establish_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_establish\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_establish_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "established"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut established = anthro_with_agents("establish review", &establish_server)
        .session("s_agent_tier_conflict")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut established, "toolu_establish").await;
    established.cancel();
    drop(established);

    let before_conflict = EventStore::replay(env.events_path("s_agent_tier_conflict")).unwrap();
    let review_opened_before = before_conflict
        .iter()
        .filter(|event| matches!(
            event.payload,
            EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review"
        ))
        .count();
    let review_bound_before = before_conflict
        .iter()
        .filter(|event| {
            matches!(
                event.payload,
                EventPayload::ConversationBound { ref conversation, .. } if conversation == "review"
            )
        })
        .count();
    let review_messages_before = before_conflict
        .iter()
        .filter(|event| {
            matches!(
                event.payload,
                EventPayload::MessageUser { ref conversation, .. } if conversation == "review"
            )
        })
        .count();

    let conflict_server = MockServer::start();
    conflict_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("tier conflict");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_conflict_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "conflict"},
                    {"type": "tool_use", "id": "toolu_conflict", "name": "agent", "input": {"to": "review", "message": "second review", "tier": "strong"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    conflict_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_conflict\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_conflict_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "conflict handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut conflict = anthro_with_agents("tier conflict", &conflict_server)
        .session("s_agent_tier_conflict")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut conflict, "toolu_conflict").await;
    conflict.cancel();

    let after_conflict = EventStore::replay(env.events_path("s_agent_tier_conflict")).unwrap();
    assert!(after_conflict.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { tool_call_id, status, summary, .. }
            if tool_call_id == "toolu_conflict"
                && status == "error"
                && summary.contains("cannot set tier when continuing existing conversation review")
    )));
    assert_eq!(
        after_conflict
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        review_opened_before
    );
    assert_eq!(
        after_conflict
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationBound { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        review_bound_before
    );
    assert_eq!(
        after_conflict
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::MessageUser { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        review_messages_before
    );
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
async fn interrupted_open_tool_blocks_resume_without_fake_result() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let provider_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_should_not_run",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "should not run"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_interrupted_open_tool_blocks";
    let mut store = EventStore::open(env.events_path(session_id)).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-06T00:00:00Z".to_string(),
            schema_version: 2,
            session_id: session_id.to_string(),
            created_at: "2026-06-06T00:00:00Z".to_string(),
            kuku_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-06-06T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            turn: 1,
            ts: "2026-06-06T00:00:02Z".to_string(),
            conversation: "main".to_string(),
            text: "run interrupted command".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            turn: 1,
            ts: "2026-06-06T00:00:03Z".to_string(),
            request_id: "req_1".to_string(),
            text: String::new(),
            thinking: None,
            input_tokens_total: None,
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-06-06T00:00:04Z".to_string(),
            conversation: None,
            tool_call_id: "toolu_interrupted".to_string(),
            request_id: "req_1".to_string(),
            index: 0,
            tool: "run_command".to_string(),
            args: serde_json::json!({"command": "printf side-effect", "timeout": 60, "brief": "side effect"}),
        })
        .unwrap();
    drop(store);

    let error = query("resume should fail before provider")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap_err();

    assert!(matches!(error, Error::InterruptedOpenTool(_)));
    let message = error.to_string();
    assert!(message.contains(session_id));
    assert!(message.contains("toolu_interrupted"));
    provider_mock.assert_hits(0);

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref tool_call_id, .. } if tool_call_id == "toolu_interrupted"
    )));
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
        .filter(|event| matches!(event.payload, EventPayload::TurnStarted { .. }))
        .count();
    let user_inputs: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::MessageUser { text, .. } => Some(text.as_str()),
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
        EventPayload::ModelError { .. } | EventPayload::TurnCompleted { .. }
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
        EventPayload::ModelError { .. } | EventPayload::TurnCompleted { .. }
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
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
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
