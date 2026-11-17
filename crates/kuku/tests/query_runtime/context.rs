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

    std::fs::remove_file(env.workspace.path().join("AGENTS.md")).unwrap();

    let second_server = MockServer::start();
    second_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("Changed tracked files:")
            .body_contains("AGENTS.md (deleted)")
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
