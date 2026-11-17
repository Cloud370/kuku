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

#[tokio::test(flavor = "current_thread")]
async fn agent_tool_result_includes_child_output_for_parent_followup() {
    let _env = TestEnv::new();
    let server = MockServer::start();

    let main_tool = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_initial_request)
            .body_contains("delegate child output");
        then.status(200)
            .header("request-id", "req_main_agent_result")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_agent_result",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Delegating to explore."},
                    {"type": "tool_use", "id": "toolu_agent_child", "name": "agent", "input": {"to": "explore", "message": "child task"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let child_done = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_initial_child_task);
        then.status(200)
            .header("request-id", "req_child_agent_result")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_child_agent_result",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "KUKU_CHILD_AGENT_RESULT"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let parent_followup = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_contains_child_agent_result);
        then.status(200)
            .header("request-id", "req_parent_after_agent_result")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_parent_after_agent_result",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "parent saw KUKU_CHILD_AGENT_RESULT"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro_with_agents("delegate child output", &server)
        .run()
        .await
        .unwrap();

    assert_eq!(output.text, "parent saw KUKU_CHILD_AGENT_RESULT");
    main_tool.assert_hits(1);
    child_done.assert_hits(1);
    parent_followup.assert_hits(1);
}

#[tokio::test(flavor = "current_thread")]
async fn agent_to_opens_nested_address_from_root_contact() {
    let env = TestEnv::new();
    let session_id = "s_agent_nested";
    let server = MockServer::start();

    let main_tool = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_initial_request)
            .body_contains("delegate nested");
        then.status(200)
            .header("request-id", "req_main_tool_nested")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_main_tool_nested",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Delegating nested review."},
                    {"type": "tool_use", "id": "toolu_agent_nested", "name": "agent", "input": {"to": "review/api", "message": "nested review"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let nested_followup = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_nested_followup")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_nested_followup",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "nested review done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 3}
            })));
    });
    let mut output = anthro_with_agents("delegate nested", &server)
        .session(session_id)
        .start()
        .await
        .unwrap();
    wait_for_tool_end(&mut output, "toolu_agent_nested").await;
    output.cancel();
    main_tool.assert_hits(1);
    let _ = nested_followup.hits();

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    let nested_kinds: Vec<&str> = events
        .iter()
        .filter(|event| event_conversation(&event.payload) == Some("review/api"))
        .map(|event| event.payload.kind_name())
        .collect();
    let opened_index = nested_kinds
        .iter()
        .position(|kind| *kind == "conversation.opened")
        .unwrap();
    let bound_index = nested_kinds
        .iter()
        .position(|kind| *kind == "conversation.bound")
        .unwrap();
    let started_index = nested_kinds
        .iter()
        .position(|kind| *kind == "turn.started")
        .unwrap();
    let user_index = nested_kinds
        .iter()
        .position(|kind| *kind == "message.user")
        .unwrap();
    assert!(opened_index < bound_index);
    assert!(bound_index < started_index);
    assert!(started_index < user_index);
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::MessageUser { conversation, from, via_tool_call_id, .. }
            if conversation == "review/api"
                && from.as_deref() == Some("main")
                && via_tool_call_id.as_deref() == Some("toolu_agent_nested")
    )));
    assert!(!tree_contains_name(env.home.path(), "subs"));
    assert!(!tree_contains_name(
        env.home.path(),
        "child_s_agent_nested_0"
    ));
}
