#[tokio::test(flavor = "current_thread")]
async fn second_turn_request_wraps_drift_notice_inside_runtime_context() {
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
            .body_matches(
                Regex::new(
                    r#"(?s).*<kuku_runtime_context>.*<kuku_system_notice>.*</kuku_system_notice>.*</kuku_runtime_context>.*"#,
                )
                .unwrap(),
            )
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
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
            text: "bootstrap main".into(),
            from: None,
            via_tool_call_id: None,
            job_delivery: false,
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
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
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
            text: "review secret transcript".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent".into()),
            job_delivery: false,
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
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
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            ts: "t0".into(),
            conversation: "main".into(),
            turn: 1,
            text: "bootstrap main".into(),
            from: None,
            via_tool_call_id: None,
            job_delivery: false,
        })
        .unwrap();
    store
        .append(EventPayload::TurnCompleted {
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
            ts: "t1".into(),
            conversation: "review".into(),
            turn: 1,
            text: "please review this".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent_review".into()),
            job_delivery: false,
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
            request_id: "req_review_2".into(),
            index: 0,
            tool: "run_command".into(),
            args: serde_json::json!({"program": "cargo", "args": ["test"], "cwd": "."}),
        })
        .unwrap();
    store
        .append(EventPayload::PermissionRequested {
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
            ts: "t2".into(),
            conversation: "review".into(),
            turn: 2,
            partial_reply: None,
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
            ts: "t3".into(),
            conversation: "explore".into(),
            turn: 1,
            text: "explore secret transcript".into(),
            from: Some("main".into()),
            via_tool_call_id: Some("toolu_agent_explore".into()),
            job_delivery: false,
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
async fn first_turn_request_includes_budgeted_skill_block_and_hints() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let skill_dir = env
        .workspace
        .path()
        .join(".claude")
        .join("skills")
        .join("tdd");
    let mut config = test_config();
    config.discovery.auto_discover = false;
    config.discovery.extra_project_paths = vec![env.workspace.path().join(".claude")];
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: tdd\ndescription: Write tests first\n---\n\nInstructions.\n",
    )
    .unwrap();

    let request = server.mock(|when, then| {
        context_conditions(when, "show skills")
            .method(POST)
            .path("/v1/messages")
            .body_contains("<kuku_skills>")
            .body_contains("Available skills: 1 total")
            .body_contains("tdd - Write tests first")
            .body_contains("Use list_skills to browse available skills.")
            .body_contains("Use search_skills to find skills by task or workflow.");
        then.status(200)
            .header("request-id", "req_skills")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_skills",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "skills ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });
    let output = query("show skills")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
        .run()
        .await
        .unwrap();

    request.assert_hits(1);
    assert_eq!(output.text, "skills ok");
}
