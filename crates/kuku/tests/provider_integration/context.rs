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
