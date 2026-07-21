#[tokio::test(flavor = "current_thread")]
async fn skill_attachment_is_conversation_scoped() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let session_id = "s_skill_attachment_scope";
    let mut config = test_config();
    config.discovery.auto_discover = false;
    config.discovery.extra_project_paths = vec![env.workspace.path().join(".claude")];

    let skills_root = env.workspace.path().join(".claude").join("skills");
    let main_skill_dir = skills_root.join("main-skill");
    let api_skill_dir = skills_root.join("api-skill");
    std::fs::create_dir_all(&main_skill_dir).unwrap();
    std::fs::create_dir_all(&api_skill_dir).unwrap();
    std::fs::write(
        main_skill_dir.join("SKILL.md"),
        "---\nname: main-skill\ndescription: Main scoped skill\n---\n\nMain skill instructions.\n",
    )
    .unwrap();
    std::fs::write(
        api_skill_dir.join("SKILL.md"),
        "---\nname: api-skill\ndescription: API scoped skill\n---\n\nAPI skill instructions.\n",
    )
    .unwrap();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "main use skill")
                    && !request_body_contains(req, "Main skill instructions.")
            });
        then.status(200).body(anthropic_sse_response(serde_json::json!({
            "id": "msg_main_tool",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "load main skill"},
                {"type": "tool_use", "id": "toolu_main_skill", "name": "use_skill", "input": {"skill_name": "main-skill"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        })));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "Main skill instructions.")
                && request_body_contains(req, "main use skill")
                && !request_body_contains(req, "main followup")
                && !request_body_contains(req, "review followup")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_done",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main loaded"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });
    let main_loaded = query("main use skill")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(main_loaded.text, "main loaded");

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "review prompt")
                && !request_body_contains(req, "review followup")
                && !request_body_contains(req, "Main skill instructions.")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review clean"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    let review_clean = query("review prompt")
        .session(session_id)
        .conversation("review")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(review_clean.text, "review clean");

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "review api use skill")
                    && !request_body_contains(req, "API skill instructions.")
            });
        then.status(200).body(anthropic_sse_response(serde_json::json!({
            "id": "msg_api_tool",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "load api skill"},
                {"type": "tool_use", "id": "toolu_api_skill", "name": "use_skill", "input": {"skill_name": "api-skill"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        })));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "API skill instructions.")
                && !request_body_contains(req, "Main skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_api_done",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "api loaded"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 5}
            })));
    });
    let api_loaded = query("review api use skill")
        .session(session_id)
        .conversation("review/api")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(api_loaded.text, "api loaded");

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "main followup")
                && request_body_contains(req, "Main skill instructions.")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "main still scoped"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    let main_followup = query("main followup")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(main_followup.text, "main still scoped");

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "review followup")
                && !request_body_contains(req, "Main skill instructions.")
                && !request_body_contains(req, "API skill instructions.")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review still clean"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    let review_followup = query("review followup")
        .session(session_id)
        .conversation("review")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
        .run()
        .await
        .unwrap();
    assert_eq!(review_followup.text, "review still clean");

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ConversationBound { conversation, .. } if conversation == "main"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ConversationBound { conversation, .. } if conversation == "review/api"
    )));
    assert!(!events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ConversationBound { conversation, .. } if conversation == "review"
    )));
}
#[tokio::test(flavor = "current_thread")]
async fn delegated_agent_request_includes_contact_card_instructions() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let mut config = test_config();
    config.tiers.insert(
        "strong".to_string(),
        kuku::config::TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            think: kuku::config::ThinkLevel::Medium,
            context_window: 200_000,
            max_output_tokens: 48_000,
            purpose: "strong".to_string(),
        },
    );
    config.providers.get_mut("anthropic").unwrap().base_url = server.base_url();

    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "delegate review")
                    && !request_body_contains(req, "<kuku_delegated_prompt>")
                    && !request_body_contains(req, "check </kuku_delegated_prompt> & <tag> > boundary")
            });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "delegating"},
                    {"type": "tool_use", "id": "toolu_review_card", "name": "agent", "input": {"to": "review", "message": "check </kuku_delegated_prompt> & <tag> > boundary"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let child_request = server.mock(|when, then| {
        when.method(POST).path("/v1/messages").matches(|req| {
            request_body_contains(req, "check </kuku_delegated_prompt> & <tag> > boundary")
                && request_body_contains(req, "I am a code and document reviewer")
        });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_card",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_review_card\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });

    let mut run = anthro_with_agents("delegate review", &server)
        .session("s_agent_contact_card")
        .config(config.clone())
        .start()
        .await
        .unwrap();
    next_tool_end(&mut run, "toolu_review_card").await;
    run.cancel();
    drop(run);

    child_request.assert();

    let events = EventStore::replay(env.events_path("s_agent_contact_card")).unwrap();
    let mut request_starts = events.iter().filter_map(|event| match &event.payload {
        EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)) => {
            batch.events().iter().find_map(|event| match event {
                TaskEvent::RequestStarted(started) => Some(started),
                _ => None,
            })
        }
        _ => None,
    });
    let delegated_start = request_starts
        .clone()
        .find(|started| matches!(started.cause, RequestCause::DelegatedAgent { .. }))
        .expect("delegated request.started");
    let RequestCause::DelegatedAgent { parent_request_id } = &delegated_start.cause else {
        unreachable!();
    };
    assert!(request_starts
        .clone()
        .any(|started| &started.scope.request_id == parent_request_id));
    let parent_start = request_starts
        .find(|started| &started.scope.request_id == parent_request_id)
        .expect("parent request.started");
    assert_eq!(
        parent_start.scope.execution.task_id,
        delegated_start.scope.execution.task_id
    );
    assert_eq!(
        parent_start.scope.execution.run_id,
        delegated_start.scope.execution.run_id
    );
    assert_ne!(
        parent_start.scope.execution.turn_id,
        delegated_start.scope.execution.turn_id
    );
    assert_ne!(
        parent_start.scope.execution.conversation_id,
        delegated_start.scope.execution.conversation_id
    );
    let child_message = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::MessageUser {
                execution: _,
                conversation,
                text,
                ..
            } if conversation == "review" => Some(text),
            _ => None,
        })
        .expect("child message.user");
    assert_eq!(
        child_message,
        "check </kuku_delegated_prompt> & <tag> > boundary"
    );
    assert!(!child_message.contains("I am a code and document reviewer"));
    assert!(!child_message.contains("<kuku_delegated_prompt>"));

    assert!(!events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::PromptSnapshot { conversation, .. } if conversation == "review"
    )));

    let second_server = MockServer::start();
    config.providers.get_mut("anthropic").unwrap().base_url = second_server.base_url();
    second_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| {
                request_body_contains(req, "delegate review again")
                    && !request_body_contains(req, "<kuku_delegated_prompt>")
            });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_again_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "delegating again"},
                    {"type": "tool_use", "id": "toolu_review_again", "name": "agent", "input": {"to": "review", "message": "second review boundary"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let second_child_request = second_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(|req| request_body_contains(req, "second review boundary"));
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_review_again",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review again done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 7, "output_tokens": 4}
            })));
    });
    second_server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_review_again\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_delegate_again_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "done again"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });

    let mut second_run = anthro_with_agents("delegate review again", &second_server)
        .session("s_agent_contact_card")
        .config(config)
        .start()
        .await
        .unwrap();
    next_tool_end(&mut second_run, "toolu_review_again").await;
    second_run.cancel();

    second_child_request.assert();
}

fn list_event_files(kuku_home: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn visit(dir: &std::path::Path, paths: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, paths);
            } else if path.file_name().is_some_and(|name| name == "events.jsonl") {
                paths.push(path);
            }
        }
    }

    let mut paths = Vec::new();
    let root = kuku_home.join("p");
    if root.exists() {
        visit(&root, &mut paths);
    }
    paths
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_is_not_polluted() {
    let env = TestEnv::new();

    let _ = query("no pollution")
        .config(test_config())
        .run()
        .await
        .unwrap_err();

    assert_eq!(std::fs::read_dir(env.workspace_path()).unwrap().count(), 0);
    assert!(!env.workspace_path().join(".kuku").exists());
    assert!(!env.workspace_path().join(".kuku-id").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_session_ids_fail_before_creating_session_path() {
    let env = TestEnv::new();

    for session_id in [
        "../bad",
        "CON",
        "con",
        "COM1",
        "LPT9",
        "CON.txt",
        "aux.log",
        "LPT1.json",
        "name.",
        "name ",
    ] {
        let error = query("bad")
            .session(session_id)
            .config(test_config())
            .run()
            .await
            .unwrap_err();
        assert!(matches!(error, Error::InvalidSessionId(ref value) if value == session_id));
    }

    assert!(!env.home.path().join("p").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn agent_tool_rejects_reserved_main_and_tier_conflict() {
    let env = TestEnv::new();

    let reserved_server = MockServer::start();
    reserved_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("reserved main");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_reserved_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "try reserved"},
                    {"type": "tool_use", "id": "toolu_reserved", "name": "agent", "input": {"to": "main", "message": "bad target"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    reserved_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_reserved\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_reserved_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "reserved handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut reserved = anthro_with_agents("reserved main", &reserved_server)
        .session("s_agent_reserved")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut reserved, "toolu_reserved").await;
    reserved.cancel();
    let reserved_events = EventStore::replay(env.events_path("s_agent_reserved")).unwrap();
    assert!(reserved_events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
    tool_call_id, status, summary, .. }
                if tool_call_id == "toolu_reserved"
                    && status == "error"
                    && summary.contains("reserved conversation address 'main'")
        )));
    assert_eq!(
        reserved_events
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationOpened { ref conversation, .. } if conversation == "main"
            ))
            .count(),
        1
    );

    let invalid_server = MockServer::start();
    invalid_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("invalid address");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_invalid_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "try invalid"},
                    {"type": "tool_use", "id": "toolu_invalid", "name": "agent", "input": {"to": "review//api", "message": "bad target"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    invalid_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_invalid\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_invalid_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "invalid handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut invalid = anthro_with_agents("invalid address", &invalid_server)
        .session("s_agent_invalid")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut invalid, "toolu_invalid").await;
    invalid.cancel();
    let invalid_events = EventStore::replay(env.events_path("s_agent_invalid")).unwrap();
    assert!(invalid_events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
    tool_call_id, status, summary, .. }
                if tool_call_id == "toolu_invalid"
                    && status == "error"
                    && summary.contains("invalid slash placement")
        )));
    assert!(!invalid_events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review//api"
    )));

    let unknown_server = MockServer::start();
    unknown_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("unknown contact");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_unknown_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "try unknown"},
                    {"type": "tool_use", "id": "toolu_unknown", "name": "agent", "input": {"to": "unknown", "message": "bad target"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    unknown_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_unknown\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_unknown_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "unknown handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut unknown = anthro_with_agents("unknown contact", &unknown_server)
        .session("s_agent_unknown")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut unknown, "toolu_unknown").await;
    unknown.cancel();
    let unknown_events = EventStore::replay(env.events_path("s_agent_unknown")).unwrap();
    assert!(unknown_events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
    tool_call_id, status, summary, .. }
                if tool_call_id == "toolu_unknown"
                    && status == "error"
                    && summary.contains("unknown agent contact: unknown")
        )));
    assert!(!unknown_events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ConversationOpened { ref conversation, .. } if conversation == "unknown"
    )));

    let establish_server = MockServer::start();
    establish_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("establish review");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_establish_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "establish"},
                    {"type": "tool_use", "id": "toolu_establish", "name": "agent", "input": {"to": "review", "message": "initial review"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    establish_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .matches(|req| {
                req.body.as_ref().is_some_and(|body| {
                    body.windows(b"initial review".len())
                        .any(|w| w == b"initial review")
                })
            });
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_establish_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "review ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    establish_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_establish\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_establish_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "established"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut established = anthro_with_agents("establish review", &establish_server)
        .session("s_agent_tier_conflict")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut established, "toolu_establish").await;
    established.cancel();
    drop(established);

    let before_conflict = EventStore::replay(env.events_path("s_agent_tier_conflict")).unwrap();
    let review_opened_before = before_conflict
        .iter()
        .filter(|event| matches!(
            event.payload,
            EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review"
        ))
        .count();
    let review_bound_before = before_conflict
        .iter()
        .filter(|event| {
            matches!(
                event.payload,
                EventPayload::ConversationBound { ref conversation, .. } if conversation == "review"
            )
        })
        .count();
    let review_messages_before = before_conflict
        .iter()
        .filter(|event| {
            matches!(
                            event.payload,
                            EventPayload::MessageUser {
            ref conversation, .. } if conversation == "review"
                        )
        })
        .count();

    let conflict_server = MockServer::start();
    conflict_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("tier conflict");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_conflict_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "conflict"},
                    {"type": "tool_use", "id": "toolu_conflict", "name": "agent", "input": {"to": "review", "message": "second review", "tier": "strong"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    conflict_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_use_id\":\"toolu_conflict\"");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_conflict_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "conflict handled"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 4}
            })));
    });
    let mut conflict = anthro_with_agents("tier conflict", &conflict_server)
        .session("s_agent_tier_conflict")
        .start()
        .await
        .unwrap();
    next_tool_end(&mut conflict, "toolu_conflict").await;
    conflict.cancel();

    let after_conflict = EventStore::replay(env.events_path("s_agent_tier_conflict")).unwrap();
    assert!(after_conflict.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult {
tool_call_id, status, summary, .. }
            if tool_call_id == "toolu_conflict"
                && status == "error"
                && summary.contains("cannot set tier when continuing existing conversation review")
    )));
    assert_eq!(
        after_conflict
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationOpened { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        review_opened_before
    );
    assert_eq!(
        after_conflict
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ConversationBound { ref conversation, .. } if conversation == "review"
            ))
            .count(),
        review_bound_before
    );
    assert_eq!(
        after_conflict
            .iter()
            .filter(|event| matches!(
                            event.payload,
                            EventPayload::MessageUser {
            ref conversation, .. } if conversation == "review"
                        ))
            .count(),
        review_messages_before
    );
}

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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
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
            matches!(event.payload, EventPayload::PermissionRequested {
ref tool_call_id, .. } if tool_call_id == "toolu_cmd")
        })
        .expect("permission.requested event");

    assert!(tool_call_pos < permission_pos);
    match &events[permission_pos].payload {
        EventPayload::PermissionRequested {
            execution: _,
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
            assert_eq!(candidate, "cargo test");
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
