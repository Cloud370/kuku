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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
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
            assert_eq!(
                serde_json::json!({"cwd":".","invocation":{"mode":"program","program":"cargo","args":["test"]}}),
                serde_json::from_str::<serde_json::Value>(&candidate).unwrap()
            );
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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
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
                    {"type": "tool_use", "id": "toolu_cmd_2", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
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
                    {"type": "tool_use", "id": "toolu_resume_cancel", "name": "run_command", "input": {"program": "printf", "args": ["should-not-run"], "cwd": ".", "timeout": 60, "brief": "print cancelled marker"}}
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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
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
    let (tool, risk, candidate, source) = requested;
    assert_eq!(
        ("run_command", "command", "default_ask"),
        (tool, risk, source)
    );
    assert_eq!(
        serde_json::json!({"cwd":".","invocation":{"args":["test"],"mode":"program","program":"cargo"}}),
        serde_json::from_str::<serde_json::Value>(candidate).unwrap()
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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"shell":if cfg!(windows) {"cmd"} else {"sh"},"command":"echo allowed","cwd":".","timeout":60,"brief":"print marker"}}
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
