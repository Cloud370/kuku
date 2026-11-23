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
    assert_eq!(10, events.len());
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSources { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSkills { .. })));
    assert!(matches!(
        events[events.len() - 2].payload,
        EventPayload::ModelResponse { .. }
    ));
    assert!(matches!(
        events[events.len() - 1].payload,
        EventPayload::TurnCompleted { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn unsigned_thinking_is_durable_across_three_turns_and_profile_flip() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let session_id = "s_unsigned_prefix";

    for (turn, base_path, prompt, thought, answer) in [
        (
            1,
            "/unsigned-history-1",
            "prefix turn one",
            "thought one",
            "answer one",
        ),
        (
            2,
            "/unsigned-history-2",
            "prefix turn two",
            "thought two",
            "answer two",
        ),
        (
            3,
            "/unsigned-history-3",
            "prefix turn three",
            "thought three",
            "answer three",
        ),
    ] {
        let thinking = if turn == 2 {
            serde_json::json!({"type":"thinking","thinking":thought,"signature":""})
        } else {
            serde_json::json!({"type":"thinking","thinking":thought})
        };
        let endpoint = format!("{base_path}/v1/messages");
        let response = anthropic_sse_response(serde_json::json!({
            "id": format!("msg_{turn}"),
            "type": "message",
            "role": "assistant",
            "content": [thinking, {"type":"text","text":answer}],
            "stop_reason": "end_turn"
        }));
        server.mock(|when, then| {
            when.method(POST)
                .path(endpoint.as_str())
                .matches(common::capture_http_request);
            then.status(200).body(response);
        });

        let output = anthro_profile(prompt, &server, base_path, ProviderProfile::Anthropic)
            .session(session_id)
            .run()
            .await
            .unwrap();
        assert_eq!(answer, output.text);
    }

    let paths = [
        "/unsigned-history-1/v1/messages",
        "/unsigned-history-2/v1/messages",
        "/unsigned-history-3/v1/messages",
    ];
    let bodies = paths
        .iter()
        .map(|path| common::take_http_requests(path).remove(0))
        .collect::<Vec<_>>();
    let stable_prefix = |body: &serde_json::Value| {
        serde_json::json!({
            "model": body["model"],
            "system": body["system"],
            "tools": body["tools"],
            "cache_control": body["cache_control"],
            "thinking": body["thinking"],
            "output_config": body["output_config"],
        })
    };
    assert_eq!(stable_prefix(&bodies[0]), stable_prefix(&bodies[1]));
    assert_eq!(stable_prefix(&bodies[1]), stable_prefix(&bodies[2]));

    let second_messages = bodies[1]["messages"].as_array().unwrap();
    let third_messages = bodies[2]["messages"].as_array().unwrap();
    assert!(second_messages.len() >= 2);
    assert!(third_messages.len() >= 2);
    assert_eq!(&second_messages[..2], &third_messages[..2]);

    let admitted_history_path = env.events_path(session_id);
    let admitted_history_bytes = std::fs::read(&admitted_history_path).unwrap();
    let events_before_flip = EventStore::replay(env.events_path(session_id)).unwrap();
    let replay_events = |events: &[kuku::event::StoredEvent]| {
        events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::Unknown(value)
                    if value["kind"] == "runtime.provider_replay"
                        && value["turn"].as_u64().is_some_and(|turn| turn <= 3) =>
                {
                    Some((value["turn"].as_u64().unwrap(), event.payload.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let admitted_before_flip = replay_events(&events_before_flip);
    assert_eq!(3, admitted_before_flip.len());
    for (turn, thought, answer) in [
        (1, "thought one", "answer one"),
        (2, "thought two", "answer two"),
        (3, "thought three", "answer three"),
    ] {
        assert!(events_before_flip.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ModelResponse {
                turn: event_turn,
                thinking: Some(thinking),
                stop_reason: Some(ModelStopReason::EndTurn),
                ..
            } if *event_turn == turn && thinking == thought
        )));
        let replay = admitted_before_flip
            .iter()
            .find_map(|(event_turn, payload)| (*event_turn == turn).then_some(payload))
            .unwrap();
        let EventPayload::Unknown(value) = replay else {
            panic!("provider replay event must remain durable");
        };
        assert_eq!(
            serde_json::json!([{"type":"text","text":answer}]),
            value["value"]["content"]
        );
    }

    let flip_path = "/unsigned-history-flip";
    let flip_endpoint = format!("{flip_path}/v1/messages");
    server.mock(|when, then| {
        when.method(POST)
            .path(flip_endpoint.as_str())
            .matches(common::capture_http_request);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_flip",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type":"thinking","thinking":"compatible thought"},
                    {"type":"text","text":"answer after flip"}
                ],
                "stop_reason": "end_turn"
            })));
    });
    let output = anthro_profile(
        "flip provider profile",
        &server,
        flip_path,
        ProviderProfile::Compatible,
    )
    .session(session_id)
    .run()
    .await
    .unwrap();
    assert_eq!("answer after flip", output.text);

    let flipped_history_bytes = std::fs::read(&admitted_history_path).unwrap();
    assert!(flipped_history_bytes.starts_with(&admitted_history_bytes));
    let events_after_flip = EventStore::replay(env.events_path(session_id)).unwrap();
    assert_eq!(admitted_before_flip, replay_events(&events_after_flip));
    let flip_replay = events_after_flip
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.provider_replay" && value["turn"] == 4 =>
            {
                Some(&value["value"]["content"])
            }
            _ => None,
        });
    assert_eq!(
        Some(&serde_json::json!([
            {"type":"thinking","thinking":"compatible thought"},
            {"type":"text","text":"answer after flip"}
        ])),
        flip_replay
    );
}

#[tokio::test(flavor = "current_thread")]
async fn unsigned_thinking_does_not_block_tool_dispatch_through_sdk() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(
        env.workspace.path().join("README.md"),
        "# Unsigned thinking",
    )
    .unwrap();
    let endpoint = "/unsigned-thinking-tool/v1/messages";
    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "find files").path(endpoint);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_unsigned_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type":"thinking","thinking":"I should inspect the workspace."},
                    {"type":"text","text":"Inspecting files."},
                    {"type":"tool_use","id":"toolu_unsigned","name":"find_files","input":{"path":"."}}
                ],
                "stop_reason": "tool_use"
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST).path(endpoint);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_unsigned_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type":"text","text":"README.md found."}],
                "stop_reason": "end_turn"
            })));
    });

    let output = anthro_profile(
        "find files",
        &server,
        "/unsigned-thinking-tool",
        ProviderProfile::Anthropic,
    )
    .run()
    .await
    .unwrap();
    tool_mock.assert();
    final_mock.assert();
    assert_eq!("README.md found.", output.text);

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ModelResponse {
            thinking: Some(thinking),
            stop_reason: Some(ModelStopReason::ToolUse),
            ..
        } if thinking == "I should inspect the workspace."
    )));
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolCall {
            tool,
            tool_call_id,
            ..
        } if tool == "find_files" && tool_call_id == "toolu_unsigned"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { status, model_content, .. }
            if status == "ok" && model_content.contains("README.md")
    )));
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::Unknown(value)
            if value["kind"] == "runtime.provider_replay"
                && value["turn"] == 1
                && value["value"]["content"]
                    == serde_json::json!([
                        {"type":"text","text":"Inspecting files."},
                        {"type":"tool_use","id":"toolu_unsigned","name":"find_files","input":{"path":"."}}
                    ])
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_length_response_is_retried_once_with_frozen_notice() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let prompts_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(prompts_dir.path().join("runtime")).unwrap();
    let recovery_path = prompts_dir.path().join("runtime/recovery.md");
    let frozen_notice = "FROZEN_RECOVERY_NOTICE_V1";
    std::fs::write(&recovery_path, frozen_notice).unwrap();
    let frozen_asset = kuku::prompt::PromptCatalog::load_from_dir(prompts_dir.path())
        .unwrap()
        .runtime["recovery"]
        .clone();
    let first = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_original_frozen_notice_request)
            .body_contains(r#""max_tokens":37"#);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_length",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "partial"}],
                "stop_reason": "max_tokens",
                "usage": {"input_tokens": 7, "output_tokens": 32}
            })));
    });
    let second = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("finish the task")
            .body_contains(frozen_notice)
            .body_contains(r#""max_tokens":37"#);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_recovered",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "completed"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 9}
            })));
    });

    let mut run = anthro("finish the task", &server)
        .max_output_tokens(37)
        .prompts_dir(prompts_dir.path())
        .start()
        .await
        .unwrap();
    let mut live_order = Vec::new();
    let output = loop {
        match run.next().await.unwrap().unwrap() {
            UiEvent::ModelRequest { request_id, .. } if request_id == "req_1" => {
                live_order.push("request_1");
                std::fs::write(&recovery_path, "MUTATED_RECOVERY_NOTICE_V2").unwrap();
            }
            UiEvent::ModelRecovery { .. } => live_order.push("recovery"),
            UiEvent::ModelRequest { request_id, .. } if request_id == "req_2" => {
                live_order.push("request_2");
            }
            UiEvent::Done { output, .. } => break output,
            _ => {}
        }
    };
    assert_eq!("completed", output.text);
    assert_eq!(2, output.model_request_count);
    assert_eq!(vec!["request_1", "recovery", "request_2"], live_order);
    first.assert();
    second.assert();
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    let response_ids = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ModelResponse { request_id, .. } => Some(request_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(vec!["req_1", "req_2"], response_ids);
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ModelRecovery {
            from_request_id,
            to_request_id,
            reason: kuku::event::ModelStopReason::Length,
            notice,
            prompt_path,
            prompt_hash,
            ..
        } if from_request_id == "req_1"
            && to_request_id == "req_2"
            && notice == frozen_notice
            && prompt_path == &frozen_asset.path
            && prompt_hash == &frozen_asset.hash
    )));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::TurnCompleted { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_transport_failure_emits_invalidation_before_error() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let first = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_original_request);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_length",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "discarded partial"}],
                "stop_reason": "max_tokens",
                "usage": {"input_tokens": 7, "output_tokens": 37}
            })));
    });
    let retry = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("previous model attempt reached its output limit");
        then.status(503).body("retry unavailable");
    });
    let session_id = "s_recovery_transport_failure";
    let mut run = anthro("finish despite a short first response", &server)
        .session(session_id)
        .max_output_tokens(37)
        .start()
        .await
        .unwrap();
    let mut live_order = Vec::new();
    let error = loop {
        match run.next().await {
            Ok(Some(UiEvent::ModelRecovery { .. })) => live_order.push("recovery"),
            Ok(Some(UiEvent::ModelRequest { request_id, .. })) if request_id == "req_2" => {
                live_order.push("request_2");
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("run ended without reporting the recovery failure"),
            Err(error) => break error,
        }
    };

    assert!(matches!(error, Error::Provider { .. }));
    assert_eq!(vec!["recovery", "request_2"], live_order);
    first.assert();
    retry.assert();
    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ModelRecovery { .. }))
            .count()
    );
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ModelError { .. }))
            .count()
    );
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::TurnInterrupted { .. }))
            .count()
    );
}

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
    assert_eq!(10, events.len());
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextSkills { .. })));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ModelResponse { ref text, .. } if text == "Hi from GPT!"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn openai_chat_second_length_interrupts_after_one_recovery() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let first = server.mock(|when, then| {
        when.method(POST)
            .path("/chat/completions")
            .matches(is_original_request)
            .body_contains(r#""max_tokens":37"#);
        then.status(200)
            .body(openai_sse_response(serde_json::json!({
                "choices": [{"message": {"content": "partial one"}, "finish_reason": "length"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 37}
            })));
    });
    let second = server.mock(|when, then| {
        when.method(POST)
            .path("/chat/completions")
            .body_contains("finish the chat task")
            .body_contains("previous model attempt reached its output limit")
            .body_contains(r#""max_tokens":37"#);
        then.status(200)
            .body(openai_sse_response(serde_json::json!({
                "choices": [{"message": {"content": "partial two"}, "finish_reason": "length"}],
                "usage": {"prompt_tokens": 4, "completion_tokens": 37}
            })));
    });

    let session_id = "s_chat_length_twice";
    let error = query("finish the chat task")
        .session(session_id)
        .provider(Provider::OpenAiCompatible)
        .model("test-chat")
        .base_url(server.base_url())
        .api_key("openai-key")
        .max_output_tokens(37)
        .config(test_config())
        .run()
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::Provider { ref message, .. }
            if message.contains("max_output_tokens=37")
                && message.contains("recovery attempt 1/1")
    ));
    first.assert();
    second.assert();
    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert_eq!(
        2,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ModelResponse { .. }))
            .count()
    );
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ModelRecovery { .. }))
            .count()
    );
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::TurnInterrupted { .. }))
            .count()
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::TurnCompleted { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn openai_responses_length_recovery_keeps_original_input_and_optional_limit() {
    for (prefix, limit) in [("/recovery-limited", Some(37)), ("/recovery-default", None)] {
        let env = TestEnv::new();
        let server = MockServer::start();
        let path = format!("{prefix}/responses");
        let first = server.mock(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(is_original_request)
                .matches(common::capture_http_request);
            then.status(200).body(responses_sse(
                "resp_length",
                "partial",
                concat!(
                    "event: response.incomplete\n",
                    "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_length\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_tokens\"},\"usage\":{\"input_tokens\":8,\"output_tokens\":37}}}\n\n",
                ),
            ));
        });
        let second = server.mock(|when, then| {
            when.method(POST)
                .path(&path)
                .body_contains("finish the responses task")
                .body_contains("previous model attempt reached its output limit")
                .matches(common::capture_http_request);
            then.status(200).body(responses_sse(
                "resp_recovered",
                "completed",
                concat!(
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_recovered\",\"status\":\"completed\",\"usage\":{\"input_tokens\":9,\"output_tokens\":6}}}\n\n",
                ),
            ));
        });

        let mut config = test_config();
        let tier = config.tiers.get_mut("balanced").unwrap();
        tier.max_output_tokens = None;
        if limit.is_some() {
            tier.context_window = 10_000;
        }
        let mut request = query("finish the responses task")
            .provider(Provider::OpenAiResponses)
            .model("test-responses")
            .base_url(server.url(prefix))
            .api_key("responses-key")
            .config(config);
        if let Some(limit) = limit {
            request = request.max_output_tokens(limit);
        }
        let output = request.run().await.unwrap();

        first.assert();
        second.assert();
        let requests = common::take_http_requests(&path);
        assert_eq!(2, requests.len());
        let expected = limit.map(|value| serde_json::json!(value));
        for body in requests {
            assert_eq!(expected.as_ref(), body.get("max_output_tokens"));
        }
        assert_eq!("completed", output.text);
        assert_eq!(2, output.model_request_count);
        assert_eq!(Some(43), output.usage.and_then(|usage| usage.output_tokens));
        let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
        assert!(events.iter().any(
            |event| matches!(event.payload, EventPayload::ModelRecovery {
            failed_max_output_tokens,
            retry_max_output_tokens,
            ..
        } if failed_max_output_tokens == limit && retry_max_output_tokens == limit)
        ));
    }
}

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
