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
    assert_eq!(events.len(), 10);
    assert_eq!(
        2,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::TaskLedger(_)))
            .count()
    );
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSources { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSkills { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::PromptSnapshot { .. })));
    assert!(matches!(
        events[events.len() - 2].payload,
        EventPayload::ModelResponse { .. }
    ));
    assert!(matches!(
        events[events.len() - 1].payload,
        EventPayload::TurnCompleted { .. }
    ));
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
        EventPayload::ToolResult {
ref status, ref model_content, .. }
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
async fn second_turn_request_uses_current_instructions_without_runtime_snapshot() {
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
            .body_contains("version two")
            .body_contains("next turn")
            .matches(|request| {
                !body_contains(request, b"Only unacknowledged drift is reported here.")
            });
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
async fn agent_directory_notice_lists_open_conversations() {
    let env = TestEnv::new();
    let session_id = "s_notice_open_conversations";

    let path = env.events_path(session_id);
    let mut store = EventStore::open(&path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-09T00:00:00Z".into(),
            schema_version: 2,
            session_id: session_id.into(),
            created_at: "2026-06-09T00:00:00Z".into(),
            kuku_version: env!("CARGO_PKG_VERSION").into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t0".into(),
            conversation: "main".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            execution: common::execution_scope(),
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            execution: common::execution_scope(),
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
            text: "bootstrap main".into(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
            execution: common::execution_scope(),
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t1".into(),
            conversation: "review".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            execution: common::execution_scope(),
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            execution: common::execution_scope(),
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
            text: "review secret transcript".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent".into()),
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
            execution: common::execution_scope(),
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
        })
        .unwrap();

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_open_conversation_summary_without_peer_transcript)
            .body_contains("Available contacts:")
            .body_contains("routing hint:")
            .body_contains("open conversations: 1")
            .body_contains("main followup");
        then.status(200)
            .header("request-id", "req_notice_main")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_notice_main",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main notice ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 6, "output_tokens": 4}
            })));
    });

    let output = anthro_with_agents("main followup", &server)
        .session(session_id)
        .run()
        .await
        .unwrap();
    assert_eq!(output.text, "main notice ok");
}

#[tokio::test(flavor = "current_thread")]
async fn agent_conversation_sees_own_notices_and_incoming_messages_only() {
    let env = TestEnv::new();
    let session_id = "s_notice_review_only";

    let path = env.events_path(session_id);
    let mut store = EventStore::open(&path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-09T00:00:00Z".into(),
            schema_version: 2,
            session_id: session_id.into(),
            created_at: "2026-06-09T00:00:00Z".into(),
            kuku_version: env!("CARGO_PKG_VERSION").into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t0".into(),
            conversation: "main".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            execution: common::execution_scope(),
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            execution: common::execution_scope(),
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
            text: "bootstrap main".into(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
            execution: common::execution_scope(),
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t1".into(),
            conversation: "review".into(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            execution: common::execution_scope(),
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
            text: "please review this".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent_review".into()),
        })
        .unwrap();
    store
        .append(EventPayload::ContextSkills {
            conversation: "review".into(),
            turn: 1,
            ts: "t1".into(),
            registry: serde_json::json!({}),
            bootstrap_loaded: vec!["review-skill".into()],
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            execution: common::execution_scope(),
            ts: "t2".into(),
            conversation: "review".into(),
            turn: 2,
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 2,
            ts: "t2".into(),
            conversation: Some("review".into()),
            tool_call_id: "toolu_cmd_review".into(),
            request: common::request_scope("req_review_2"),
            index: 0,
            tool: "run_command".into(),
            args: serde_json::json!({"command": "cargo test"}),
        })
        .unwrap();
    store
        .append(EventPayload::PermissionRequested {
            execution: common::execution_scope(),
            turn: 2,
            ts: "t2".into(),
            tool_call_id: "toolu_cmd_review".into(),
            tool: "run_command".into(),
            risk: "command".into(),
            summary: "run gated command".into(),
            candidate: "cargo test".into(),
            source: "default_ask".into(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnInterrupted {
            execution: common::execution_scope(),
            ts: "t2".into(),
            conversation: "review".into(),
            turn: 2,
            reason: "host_cancelled".into(),
        })
        .unwrap();
    store
        .append(EventPayload::ConversationOpened {
            ts: "t3".into(),
            conversation: "explore".into(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            execution: common::execution_scope(),
            ts: "t3".into(),
            conversation: "explore".into(),
            turn: 1,
            text: "explore secret transcript".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent_explore".into()),
        })
        .unwrap();

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_review_conversation_notices_only)
            .body_contains("review followup");
        then.status(200)
            .header("request-id", "req_notice_review")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_notice_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review notice ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 6, "output_tokens": 4}
            })));
    });

    let output = anthro_with_agents("review followup", &server)
        .session(session_id)
        .conversation("review")
        .run()
        .await
        .unwrap();
    assert_eq!(output.text, "review notice ok");
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_snapshot_is_conversation_scoped() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_main_not_review);
        then.status(200)
            .header("request-id", "req_main")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let first = query("main snapshot")
        .session("s_snapshot_scope")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "main ok");

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_review_not_main);
        then.status(200)
            .header("request-id", "req_review")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let second = query("review snapshot")
        .session("s_snapshot_scope")
        .conversation("review")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(second.text, "review ok");
}

#[tokio::test(flavor = "current_thread")]
async fn non_main_provider_request_replays_previous_assistant_reply() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    let first_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("first review request")
            .matches(|req| !body_contains(req, b"review followup"));
        then.status(200)
            .header("request-id", "req_review_first")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_first",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "previous review answer"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let first = anthro("first review request", &server)
        .session("s_review_assistant_history")
        .conversation("review")
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "previous review answer");
    first_mock.assert();

    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_review_assistant_history);
        then.status(200)
            .header("request-id", "req_review_second")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_second",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review followup ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let second = anthro("review followup", &server)
        .session("s_review_assistant_history")
        .conversation("review")
        .run()
        .await
        .unwrap();

    assert_eq!(second.text, "review followup ok");
    second_mock.assert();
}

#[tokio::test(flavor = "current_thread")]
async fn provider_request_uses_snapshot_then_history_then_current_input_frame() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    let bootstrap = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_first_input_not_live_input);
        then.status(200)
            .header("request-id", "req_bootstrap")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_bootstrap",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "assistant history reply"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let first = query("first input")
        .session("s_snapshot_order")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "assistant history reply");

    let ordered = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(snapshot_history_and_input_are_in_order);
        then.status(200)
            .header("request-id", "req_ordered")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_ordered",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ordered ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let second = query("live input")
        .session("s_snapshot_order")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(second.text, "ordered ok");

    bootstrap.assert_hits(1);
    ordered.assert_hits(1);
}

#[tokio::test(flavor = "current_thread")]
async fn agent_to_reuses_conversation_address() {
    let env = TestEnv::new();
    let session_id = "s_agent_reuse";

    let server1 = MockServer::start();
    let main_tool_1 = server1.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_initial_request)
            .body_contains("delegate first");
        then.status(200)
            .header("request-id", "req_main_tool_1")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_tool_1",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Delegating first review."},
                    {"type": "tool_use", "id": "toolu_agent_1", "name": "agent", "input": {"to": "review", "message": "review work one"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let followup_1 = server1.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_followup_1")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_followup_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review one done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let mut first = anthro_with_agents("delegate first", &server1)
        .session(session_id)
        .start()
        .await
        .unwrap();
    wait_for_tool_end(&mut first, "toolu_agent_1").await;
    first.cancel();
    main_tool_1.assert_hits(1);
    let _ = followup_1.hits();
    drop(first);

    let server2 = MockServer::start();
    let main_tool_2 = server2.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("delegate second");
        then.status(200)
            .header("request-id", "req_main_tool_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_tool_2",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Delegating second review."},
                    {"type": "tool_use", "id": "toolu_agent_2", "name": "agent", "input": {"to": "review", "message": "review work two"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let followup_2 = server2.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_followup_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_followup_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review two done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let mut second = anthro_with_agents("delegate second", &server2)
        .session(session_id)
        .start()
        .await
        .unwrap();
    wait_for_tool_end(&mut second, "toolu_agent_2").await;
    second.cancel();
    main_tool_2.assert_hits(1);
    let _ = followup_2.hits();

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationBound { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        1
    );
    let review_messages: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::MessageUser {
                execution: _,
                conversation,
                from,
                via_tool_call_id,
                ..
            } if conversation == "review" => Some((
                from.as_deref().unwrap_or(""),
                via_tool_call_id.as_deref().unwrap_or(""),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        review_messages,
        vec![("main", "toolu_agent_1"), ("main", "toolu_agent_2")]
    );
    assert!(!tree_contains_name(env.home.path(), "subs"));
    assert!(!tree_contains_name(
        env.home.path(),
        "child_s_agent_reuse_0"
    ));
    assert!(!tree_contains_name(
        env.home.path(),
        "child_s_agent_reuse_1"
    ));
}
