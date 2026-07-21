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
                execution: _,
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
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ToolResult {
ref status, .. } if status == "blocked")));
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
            matches!(event.payload, EventPayload::PermissionRequested {
ref tool_call_id, .. } if tool_call_id == "toolu_read_denied")
        })
        .expect("permission.requested event");
    let deny_pos = events
        .iter()
        .position(|event| {
            matches!(event.payload, EventPayload::PermissionDeny {
ref tool_call_id, .. } if tool_call_id == "toolu_read_denied")
        })
        .expect("permission.deny event");

    assert!(request_pos < deny_pos);
    match &events[request_pos].payload {
        EventPayload::PermissionRequested {
            execution: _,
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
    assert!(events.iter().any(
        |event| matches!(event.payload, EventPayload::PermissionAllow {
ref scope, .. } if scope == "once")
    ));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ToolResult {
ref status, .. } if status == "ok")));
}

#[tokio::test(flavor = "current_thread")]
async fn new_top_level_turn_uses_current_changed_instructions_without_runtime_snapshot() {
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
            .body_contains("version two")
            .body_contains("second turn")
            .matches(|request| {
                !request_body_contains(request, "Only unacknowledged drift is reported here.")
                    && !request_body_contains(request, "version one")
            });
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
async fn new_top_level_turn_omits_deleted_instructions_without_runtime_snapshot() {
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

    std::fs::remove_file(env.workspace.path().join("AGENTS.md")).unwrap();

    let second_server = MockServer::start();
    second_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("second turn")
            .matches(|request| {
                !request_body_contains(request, "version one")
                    && !request_body_contains(
                        request,
                        "Only unacknowledged drift is reported here.",
                    )
                    && !request_body_contains(request, "AGENTS.md (deleted)")
            });
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
            .body_contains("<kuku_shared_style>");
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
