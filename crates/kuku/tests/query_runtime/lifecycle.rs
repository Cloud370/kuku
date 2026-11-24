#[tokio::test(flavor = "current_thread")]
async fn start_creates_session_events_under_kuku_home() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(start_creates_session_events_under_kuku_home),
    ) else {
        return;
    };

    let run = query("inspect this project")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let session_id = run.session_id().to_string();

    let events = EventStore::replay(env.events_path(&session_id)).unwrap();
    assert_eq!(
        events.iter().map(|event| event.id).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );

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

    match &events[2].payload {
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

    match &events[3].payload {
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
async fn standalone_query_records_turn_and_user_input() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(standalone_query_records_turn_and_user_input),
    ) else {
        return;
    };

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
            "turn.started",
            "message.user",
            "context.skills",
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn start_persists_session_scoped_log_without_event_payload() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(start_persists_session_scoped_log_without_event_payload),
    ) else {
        return;
    };

    let run = query("inspect this project")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let session_id = run.session_id().to_string();

    let session_log_path = env
        .home
        .as_path()
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
    assert_eq!(events.len(), 4);
    assert!(!events.iter().any(|event| {
        let payload = serde_json::to_value(&event.payload).unwrap();
        payload.get("log").is_some() || payload.get("debug").is_some()
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn run_without_config_fails_before_writing_events() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(run_without_config_fails_before_writing_events),
    ) else {
        return;
    };

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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(builder_only_provider_config_starts_without_file_config),
    ) else {
        return;
    };
    let server = MockServer::start();
    let request = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""max_tokens":1024"#);
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
        .max_output_tokens(1024)
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
    request.assert();
    let events = EventStore::replay(env.events_path("s_builder_only")).unwrap();
    assert!(!events.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_session_start_appends_turn_without_duplicate_meta() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(explicit_session_start_appends_turn_without_duplicate_meta),
    ) else {
        return;
    };

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
    assert_eq!(events.len(), 8);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::SessionCreated { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events[1].payload,
        EventPayload::TurnStarted { turn: 1, .. }
    ));
    assert!(matches!(
        events[2].payload,
        EventPayload::MessageUser { turn: 1, .. }
    ));
    assert!(matches!(
        events[3].payload,
        EventPayload::ContextSkills { turn: 1, .. }
    ));
    assert!(matches!(
        events[4].payload,
        EventPayload::TurnInterrupted { turn: 1, .. }
    ));
    assert!(matches!(
        events[5].payload,
        EventPayload::TurnStarted { turn: 2, .. }
    ));
    assert!(
        !matches!(events[5].payload, EventPayload::MessageUser { .. }),
        "expected turn.started, got second message.user position"
    );
    assert!(matches!(
        events[6].payload,
        EventPayload::MessageUser { turn: 2, .. }
    ));
    match &events[7].payload {
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
    match &events[6].payload {
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(resume_marks_unterminated_main_turn_interrupted),
    ) else {
        return;
    };
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
            job_delivery: false,
            images: Vec::new(),
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
            "turn.started",
            "message.user",
            "turn.interrupted",
            "turn.started",
            "message.user",
            "context.skills",
        ]
    );
    assert!(matches!(
        &events[3].payload,
        EventPayload::TurnInterrupted { conversation, turn: 1, .. } if conversation == "main"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_is_not_polluted() {
    let Some(env) = TestEnv::enter(module_path!(), stringify!(workspace_is_not_polluted)) else {
        return;
    };

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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(invalid_session_ids_fail_before_creating_session_path),
    ) else {
        return;
    };

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

    assert!(!env.home.as_path().join("p").exists());
}
