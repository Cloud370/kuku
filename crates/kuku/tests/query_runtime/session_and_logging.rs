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
            execution: _,
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
            execution: _,
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
            execution: common::execution_scope(),
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
            execution: common::execution_scope(),
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
            execution: common::execution_scope(),
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
            execution: common::execution_scope(),
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
    let run_id = run.run_id().to_string();

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
            && record.run_id.as_deref() == Some(run_id.as_str())
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
    let run_id = run.run_id().to_string();
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
            assert_eq!(record.run_id.as_deref(), Some(run_id.as_str()));
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
                execution: _,
                conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnCancelled {
                execution: _,
                conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnInterrupted {
                execution: _,
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
            execution: _,
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
            execution: common::execution_scope(),
            ts: "2026-06-09T00:00:02Z".to_string(),
            conversation: "main".to_string(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            execution: common::execution_scope(),
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
            EventPayload::TurnInterrupted {
    conversation, turn: 1, .. } if conversation == "main"
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
    std::fs::create_dir_all(prompts_dir.path().join("blocks")).unwrap();
    std::fs::write(
        prompts_dir.path().join("blocks/project-policy.md"),
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
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { .. })));
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
