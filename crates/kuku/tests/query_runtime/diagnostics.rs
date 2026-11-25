#[tokio::test(flavor = "current_thread")]
async fn standalone_run_cancellation_interrupts_initial_stream_preflight() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    let Some(_env) = TestEnv::enter(
        module_path!(),
        stringify!(standalone_run_cancellation_interrupts_initial_stream_preflight),
    ) else {
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (headers_tx, headers_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0];
            if socket.read_exact(&mut byte).is_err() {
                return;
            }
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
            )
            .unwrap();
        let _ = headers_tx.send(());
        let _ = release_rx.recv_timeout(Duration::from_secs(3));
        let _ = socket.write_all(b"0\r\n\r\n");
    });

    let mut run = query("cancel stalled stream")
        .connection("anthropic")
        .model("claude-sonnet-4-6")
        .base_url(base_url)
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let cancel_token = run.cancel_token();
    let next = tokio::spawn(async move { run.next().await });

    tokio::time::timeout(Duration::from_secs(2), headers_rx)
        .await
        .expect("provider response headers")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    cancel_token.notify_waiters();

    let event = tokio::time::timeout(Duration::from_secs(2), next)
        .await
        .expect("cancellation should interrupt preflight")
        .unwrap()
        .unwrap();
    assert!(matches!(event, Some(UiEvent::Done { .. })));
    let _ = release_tx.send(());
    server.join().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_logs_are_fanned_out_without_events_payloads_or_immediate_info_flush() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(runtime_logs_are_fanned_out_without_events_payloads_or_immediate_info_flush),
    ) else {
        return;
    };
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200).body(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"stream overloaded\"}}\n\n",
        );
    });

    let mut run = query("emit diagnostics")
        .connection("anthropic")
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let session_id = run.session_id().to_string();
    let log_dir = env.home.as_path().join("logs").join("runtime");
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(run_convenience_path_persists_buffered_runtime_info_logs_on_completion),
    ) else {
        return;
    };
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
        .connection("anthropic")
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    let log_dir = env.home.as_path().join("logs").join("runtime");
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
    let Some(_env) = TestEnv::enter(
        module_path!(),
        stringify!(runtime_log_preserves_turn_model_log_event_order),
    ) else {
        return;
    };
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
        .connection("anthropic")
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(provider_start_failure_still_delivers_runtime_model_request_log),
    ) else {
        return;
    };
    let server = MockServer::start();

    let mut run = query("provider fails")
        .connection("anthropic")
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();
    let session_id = run.session_id().to_string();
    let log_dir = env.home.as_path().join("logs").join("runtime");

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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(context_too_large_failure_still_delivers_runtime_model_request_log),
    ) else {
        return;
    };
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(413).body("context too large");
    });

    let mut run = query("context overflow")
        .connection("anthropic")
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

#[tokio::test(flavor = "current_thread")]
async fn truncated_provider_stream_is_recorded_as_failed_turn() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(truncated_provider_stream_is_recorded_as_failed_turn),
    ) else {
        return;
    };
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200).body(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_truncated\",\"type\":\"message\",\"role\":\"assistant\",\"usage\":{\"input_tokens\":7}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        );
    });

    let mut run = query("trigger truncation")
        .connection("anthropic")
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
                        kind: kuku::ProviderFailureKind::InvalidResponse,
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
async fn provider_error_writes_single_terminal_event() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(provider_error_writes_single_terminal_event),
    ) else {
        return;
    };
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(500).body("provider exploded");
    });

    let mut run = query("provider failure")
        .connection("anthropic")
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(prompt_render_error_writes_single_terminal_event),
    ) else {
        return;
    };
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
    let session_entries = list_event_files(env.home.as_path());
    assert_eq!(session_entries.len(), 1);
    let events = EventStore::replay(&session_entries[0]).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { turn: 1, .. })));
    assert_single_terminal_kind(&events, 1, "turn.interrupted");
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_terminal_writes_for_same_turn_are_ignored() {
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(duplicate_terminal_writes_for_same_turn_are_ignored),
    ) else {
        return;
    };
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(500).body("provider exploded");
    });

    let mut run = query("provider failure")
        .connection("anthropic")
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
