// ---------------------------------------------------------------------------
// permission — denied (auto-deny when no prior grant)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn records_denied_run_command_and_continues() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "run tests")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
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
                "content": [{"type": "text", "text": "Command was blocked."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("run tests", &server).run().await.unwrap();

    tool_mock.assert();
    catch_all.assert();
    assert_eq!(output.text, "Command was blocked.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "run_command" && tool_call_id == "toolu_cmd"
    )));
    assert!(events.iter().any(|event| matches!(
            event.payload,
            EventPayload::PermissionDeny {
    ref tool_call_id, ref tool, .. }
                if tool_call_id == "toolu_cmd" && tool == "run_command"
        )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult {
ref status, ref model_content, .. }
            if status == "blocked"
                && model_content.contains("run_command was not executed because the permission gate denied this tool call")
    )));
}

// ---------------------------------------------------------------------------
// openai
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn openai_success_returns_text_and_writes_events() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/chat/completions")
            .header("authorization", "Bearer openai-key");
        then.status(200)
            .body(openai_sse_response(serde_json::json!({
                "choices": [{"message": {"content": "Hi from GPT!"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 4}
            })));
    });

    let output = query("hi")
        .provider(Provider::OpenAiCompatible)
        .model("gpt-5.4-mini")
        .base_url(server.base_url())
        .api_key("openai-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    mock.assert();
    assert_eq!(output.text, "Hi from GPT!");
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
        .any(|event| matches!(event.payload, EventPayload::ContextSkills { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::PromptSnapshot { .. })));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelResponse { ref text, .. } if text == "Hi from GPT!"
    )));
}

// ---------------------------------------------------------------------------
// error handling
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn http_error_writes_model_error_and_turn_end() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let sid = "s_http_err";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(401)
            .header("request-id", "req_http_error")
            .body("unauthorized");
    });

    let err = query("test")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("bad")
        .session(sid)
        .config(test_config())
        .run()
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Provider { .. }));

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::TurnInterrupted { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn missing_config_fails_before_writing_session_events() {
    let env = TestEnv::new();
    let sid = "s_no_cfg";

    let err = query("test").session(sid).run().await.unwrap_err();
    assert!(matches!(err, Error::MissingProviderConfig(_)));

    let events_path = env.events_path(sid);
    if events_path.exists() {
        let events = EventStore::replay(&events_path).unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::ModelError { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::TurnInterrupted { .. })));
    }
}

// ---------------------------------------------------------------------------
// security
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn api_key_is_not_written_to_events() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let sid = "s_no_leak";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })));
    });

    query("test")
        .provider(Provider::Anthropic)
        .model("m")
        .api_key("secret-123")
        .base_url(server.base_url())
        .session(sid)
        .config(test_config())
        .run()
        .await
        .unwrap();

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    let raw = format!("{events:?}");
    assert!(!raw.contains("secret-123"));
}
