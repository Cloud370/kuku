#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_reemits_request_before_new_turn() {
    let Some(_env) = TestEnv::enter(
        module_path!(),
        stringify!(pending_permission_resume_reemits_request_before_new_turn),
    ) else {
        return;
    };
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
                    {"type": "tool_use", "id": "toolu_resume_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_permission_request";
    let mut run = query("run tests")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(interrupted_open_tool_blocks_resume_without_fake_result),
    ) else {
        return;
    };
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
            job_delivery: false,
            images: Vec::new(),
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            conversation: None,
            turn: 1,
            ts: "2026-06-06T00:00:03Z".to_string(),
            request_id: "req_1".to_string(),
            text: String::new(),
            thinking: None,
            stop_reason: None,
            input_tokens_total: None,
            output_tokens_total: None,
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
            args: serde_json::json!({"program": "printf", "args": ["side-effect"], "cwd": ".", "timeout": 60, "brief": "side effect"}),
        })
        .unwrap();
    drop(store);

    let error = query("resume should fail before provider")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(pending_permission_resume_does_not_append_new_turn_before_decision),
    ) else {
        return;
    };
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
                    {"type": "tool_use", "id": "toolu_no_turn_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_no_new_turn";
    let mut run = query("run tests")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(pending_permission_resume_does_not_resolve_config_or_append_facts_before_reemit),
    ) else {
        return;
    };
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
                    {"type": "tool_use", "id": "toolu_no_resolve", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_no_resolve";
    let mut run = query("run tests")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(pending_permission_resume_decide_continues_without_duplicate_turn_or_request_id),
    ) else {
        return;
    };
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("host denied tool action");
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
                    {"type": "tool_use", "id": "toolu_resume_decide", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_decide_continues";
    let mut run = query("run tests")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .decide(&request.id, PermissionChoice::Deny)
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(pending_permission_resume_allow_executes_original_tool),
    ) else {
        return;
    };
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
                    {"type": "tool_use", "id": "toolu_resume_allow", "name": "run_command", "input": {"shell":if cfg!(windows) {"cmd"} else {"sh"},"command":"echo resumed-allowed","cwd":".","timeout":60,"brief":"print resumed marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_allow_executes";
    let mut run = query("run resumed command")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .decide(&request.id, PermissionChoice::Once)
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
    let Some(env) = TestEnv::enter(
        module_path!(),
        stringify!(pending_permission_resume_deny_records_real_deny),
    ) else {
        return;
    };
    let server = MockServer::start();

    let final_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("host denied tool action");
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
                    {"type": "tool_use", "id": "toolu_resume_deny", "name": "run_command", "input": {"program": "printf", "args": ["should-not-run"], "cwd": ".", "timeout": 60, "brief": "print denied marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_deny_records";
    let mut run = query("deny resumed command")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
        .decide(&request.id, PermissionChoice::Deny)
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
            if tool_call_id == "toolu_resume_deny" && status == "blocked" && model_content.contains("host denied tool action")
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_permission_resume_preserves_sibling_queued_permission() {
    let Some(_env) = TestEnv::enter(
        module_path!(),
        stringify!(pending_permission_resume_preserves_sibling_queued_permission),
    ) else {
        return;
    };
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
                    {"type": "tool_use", "id": "toolu_resume_first", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}},
                    {"type": "tool_use", "id": "toolu_resume_second", "name": "run_command", "input": {"program": "cargo", "args": ["check"], "cwd": ".", "timeout": 60, "brief": "run check"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_resume_sibling_permission";
    let mut run = query("run tests then check")
        .session(session_id)
        .permission_mode(kuku::permission::PermissionMode::Ask)
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
            workspace: None,
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
        .decide(&resumed_first.id, PermissionChoice::Deny)
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
