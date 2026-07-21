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
            EventPayload::MessageUser {
    conversation, from, via_tool_call_id, .. }
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

#[tokio::test(flavor = "current_thread")]
async fn executes_list_skills_and_continues_to_final_response() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let mut config = test_config();
    config.discovery.auto_discover = false;
    config.discovery.extra_project_paths = vec![env.workspace.path().join(".claude")];
    let skill_dir = env
        .workspace
        .path()
        .join(".claude")
        .join("skills")
        .join("review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review code carefully\n---\n\nReview instructions.\n",
    )
    .unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "browse skills")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_list_skills")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will list skills."},
                    {"type": "tool_use", "id": "toolu_list_skills", "name": "list_skills", "input": {"limit": 5}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Listed skills."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = query("browse skills")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(config)
        .run()
        .await
        .unwrap();

    tool_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Listed skills.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "list_skills" && tool_call_id == "toolu_list_skills"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult {
            execution: _,
            ref status,
            ref model_content,
            ref structured,
            ..
        } if status == "ok"
            && model_content.contains("review")
            && structured.as_ref().is_some_and(|value| {
                value["skills"]
                    .as_array()
                    .is_some_and(|skills| skills.iter().any(|skill| skill["name"] == "review"))
            })
    )));
}

// ---------------------------------------------------------------------------
// tool loop — multi-tool auto-execute
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn executes_read_file_and_search_text() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(
        env.workspace.path().join("README.md"),
        "# Project\nTODO root\nDone\n",
    )
    .unwrap();
    std::fs::create_dir_all(env.workspace.path().join("docs")).unwrap();
    std::fs::write(env.workspace.path().join("docs/tools.md"), "TODO docs\n").unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "read and search")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read and search."},
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md", "limit": 2}},
                    {"type": "tool_use", "id": "toolu_search", "name": "search_text", "input": {"pattern": "TODO", "view": "lines"}}
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
                "content": [{"type": "text", "text": "Read and search complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read and search", &server).run().await.unwrap();

    tool_mock.assert();
    catch_all.assert();
    assert_eq!(output.text, "Read and search complete.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "read_file" && tool_call_id == "toolu_read"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, ref tool_call_id, .. }
            if tool == "search_text" && tool_call_id == "toolu_search"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult {
ref status, ref model_content, ref structured, .. }
            if status == "ok"
                && model_content.contains("1\t# Project")
                && structured.as_ref().is_some_and(|value| value["kind"] == "file_content" && value["read_event_id"].as_u64().is_some())
    )));
    assert!(events.iter().any(|event| matches!(
            event.payload,
            EventPayload::ToolResult {
    ref status, ref model_content, ref structured, .. }
                if status == "ok"
                    && model_content.contains("README.md:2: TODO root")
                    && structured.as_ref().is_some_and(|value| value["kind"] == "search_results")
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

#[tokio::test(flavor = "current_thread")]
async fn each_slot_read_file_persists_its_own_read_event_id() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "# Project\n").unwrap();
    std::fs::create_dir_all(env.workspace.path().join("docs")).unwrap();
    std::fs::write(env.workspace.path().join("docs/tools.md"), "# Tools\n").unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "read two files")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read both files."},
                    {"type": "tool_use", "id": "toolu_read_1", "name": "read_file", "input": {"path": "README.md"}},
                    {"type": "tool_use", "id": "toolu_read_2", "name": "read_file", "input": {"path": "docs/tools.md"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Reads complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read two files", &server).run().await.unwrap();

    tool_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Reads complete.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    let read_events = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolResult {
                execution: _,
                status,
                structured: Some(structured),
                ..
            } if status == "ok" && structured["kind"] == "file_content" => {
                Some((event.id, structured))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(read_events.len(), 2);
    for (event_id, structured) in read_events {
        assert_eq!(structured["read_event_id"], event_id);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn read_file_snapshot_allows_following_edit_file() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "alpha\nbeta\n").unwrap();

    let read_mock = server.mock(|when, then| {
        context_conditions(when, "read then edit")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_read")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_read",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read the file first."},
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let edit_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(has_read_tool_result_only);
        then.status(200)
            .header("request-id", "req_edit")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_edit",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Now I can edit it."},
                    {"type": "tool_use", "id": "toolu_edit", "name": "edit_file", "input": {"path": "README.md", "old_text": "beta", "new_text": "gamma", "brief": "rename beta"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 7, "output_tokens": 8}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(has_edit_tool_result);
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Edit complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read then edit", &server).run().await.unwrap();

    read_mock.assert();
    edit_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Edit complete.");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    let read_event = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolResult {
                execution: _,
                status,
                structured: Some(structured),
                ..
            } if status == "ok" && structured["kind"] == "file_content" => {
                Some((event.id, structured))
            }
            _ => None,
        })
        .expect("missing read_file tool result");
    assert_eq!(read_event.1["path"], "README.md");
    assert_eq!(read_event.1["read_event_id"], read_event.0);
    assert!(read_event.1["read_event_id"].as_u64().unwrap() > 0);
    assert!(events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
    status, structured: Some(structured), .. }
                if status == "ok" && structured["kind"] == "file_edit"
        )));
    assert!(!events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
    status, model_content, .. }
                if status == "error"
                    && model_content.contains("prior successful read_file snapshot")
        )));
    assert_eq!(
        std::fs::read_to_string(env.workspace.path().join("README.md")).unwrap(),
        "alpha\ngamma\n"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn same_batch_read_file_then_edit_file_succeeds() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "alpha\nbeta\n").unwrap();

    let tool_mock = server.mock(|when, then| {
        context_conditions(when, "read then edit same batch")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will read and then edit the file."},
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md"}},
                    {"type": "tool_use", "id": "toolu_edit", "name": "edit_file", "input": {"path": "README.md", "old_text": "beta", "new_text": "gamma", "brief": "rename beta"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let final_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(has_read_and_edit_tool_results);
        then.status(200)
            .header("request-id", "req_final")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Same-batch edit complete."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("read then edit same batch", &server)
        .run()
        .await
        .unwrap();

    tool_mock.assert();
    final_mock.assert();
    assert_eq!(output.text, "Same-batch edit complete.");
    assert_eq!(
        std::fs::read_to_string(env.workspace.path().join("README.md")).unwrap(),
        "alpha\ngamma\n"
    );

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
    status, structured: Some(structured), .. }
                if status == "ok" && structured["kind"] == "file_edit"
        )));
    assert!(!events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
    status, model_content, .. }
                if status == "error"
                    && model_content.contains("prior successful read_file snapshot")
        )));
}

// ---------------------------------------------------------------------------
// permission — allow (streaming)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn can_allow_run_command_once_via_run_decide() {
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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "cargo test --version", "timeout": 60, "brief": "check test version"}}
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
                "content": [{"type": "text", "text": "Command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let mut run = anthro("run tests", &server).start().await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let event = next_matching(&mut run, deadline, |e| {
        matches!(e, UiEvent::PermissionRequested { .. })
    })
    .await;
    let request = match event {
        UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    };

    run.decide(&request.id, kuku::query::PermissionChoice::Session, None)
        .await
        .unwrap();

    let event = next_matching(&mut run, deadline, |e| matches!(e, UiEvent::Done { .. })).await;
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "Command completed."),
        _ => unreachable!(),
    }

    tool_mock.assert();
    catch_all.assert();

    let events = EventStore::replay(env.events_path(run.session_id())).unwrap();
    assert!(events.iter().any(|event| matches!(
            event.payload,
            EventPayload::PermissionAllow {
    ref scope, .. }
                if scope == "session"
        )));
    assert!(events.iter().any(|event| matches!(
            event.payload,
            EventPayload::ToolResult {
    ref status, .. } if status == "ok"
        )));
}

// ---------------------------------------------------------------------------
// permission — project scope persistence
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn project_scope_allow_persists_to_policy_file_and_applies_on_next_run() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let tool_mock_1 = server.mock(|when, then| {
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
    let catch_all_1 = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final_1")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "First command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let mut run = anthro("run tests", &server).start().await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let event = next_matching(&mut run, deadline, |e| {
        matches!(e, UiEvent::PermissionRequested { .. })
    })
    .await;
    let request = match event {
        UiEvent::PermissionRequested { request } => request,
        _ => unreachable!(),
    };
    run.decide(&request.id, kuku::query::PermissionChoice::Project, None)
        .await
        .unwrap();
    let event = next_matching(&mut run, deadline, |e| matches!(e, UiEvent::Done { .. })).await;
    match event {
        UiEvent::Done { output, .. } => assert_eq!(output.text, "First command completed."),
        _ => unreachable!(),
    }

    tool_mock_1.assert();
    catch_all_1.assert();

    // Policy file persisted on disk.
    let policy_path = kuku::session::project_policy_path(
        env.home.path(),
        &std::fs::canonicalize(env.workspace.path()).unwrap(),
    )
    .unwrap();
    let policy_text = std::fs::read_to_string(&policy_path).unwrap();
    assert!(policy_text.contains("run_command(cargo test)"));

    // Second run — permission auto-allowed from persisted policy.
    let server2 = MockServer::start();

    let tool_mock_2 = server2.mock(|when, then| {
        context_conditions(when, "run tests")
            .method(POST)
            .path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_tool_2",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will run a command again."},
                    {"type": "tool_use", "id": "toolu_cmd_2", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    let catch_all_2 = server2.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_final_2")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_final_2",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Second command completed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("run tests", &server2).run().await.unwrap();

    tool_mock_2.assert();
    catch_all_2.assert();
    assert_eq!(output.text, "Second command completed.");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
            event.payload,
            EventPayload::PermissionAllow {
    ref scope, .. }
                if scope == "project"
        )));
}
