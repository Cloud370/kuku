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
                "content": [{"type": "text", "text": "I found README.md and the src directory."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })));
    });

    let output = anthro("find files", &server).run().await.unwrap();

    tool_mock.assert();
    catch_all.assert();
    assert_eq!(output.text, "I found README.md and the src directory.");

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
        EventPayload::ToolResult { ref status, ref model_content, .. }
            if status == "ok" && model_content.lines().eq(["src/", "README.md"])
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
        EventPayload::ToolResult { ref status, ref model_content, ref structured, .. }
            if status == "ok"
                && model_content.contains("1\t# Project")
                && structured.as_ref().is_some_and(|value| value["kind"] == "file_content" && value["read_event_id"].as_u64().is_some())
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, ref model_content, ref structured, .. }
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
async fn read_file_range_disambiguates_a_following_edit() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(
        env.workspace.path().join("README.md"),
        "alpha\nbeta\nbeta\n",
    )
    .unwrap();

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
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md", "offset": 2, "limit": 1}}
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
    assert_eq!(false, read_event.1["is_full_file_snapshot"]);
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { status, structured: Some(structured), .. }
            if status == "ok" && structured["kind"] == "file_edit"
    )));
    assert_eq!(
        "alpha\ngamma\nbeta\n",
        std::fs::read_to_string(env.workspace.path().join("README.md")).unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn same_batch_read_file_range_disambiguates_edit() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(
        env.workspace.path().join("README.md"),
        "alpha\nbeta\nbeta\n",
    )
    .unwrap();

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
                    {"type": "tool_use", "id": "toolu_read", "name": "read_file", "input": {"path": "README.md", "offset": 3, "limit": 1}},
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
        "alpha\nbeta\ngamma\n",
        std::fs::read_to_string(env.workspace.path().join("README.md")).unwrap()
    );

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { status, structured: Some(structured), .. }
            if status == "ok" && structured["kind"] == "file_edit"
    )));
}

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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test", "--version"], "cwd": ".", "timeout": 60, "brief": "check test version"}}
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

    run.decide(&request.id, kuku::query::PermissionChoice::Session)
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
        EventPayload::PermissionAllow { ref scope, .. }
            if scope == "session"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, .. } if status == "ok"
    )));
}

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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
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
    run.decide(&request.id, kuku::query::PermissionChoice::Project)
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
    assert!(policy_text.contains(&format!("run_command({})", request.candidate)));

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
                    {"type": "tool_use", "id": "toolu_cmd_2", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
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
        EventPayload::PermissionAllow { ref scope, .. }
            if scope == "project"
    )));
}

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
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"program": "cargo", "args": ["test"], "cwd": ".", "timeout": 60, "brief": "run tests"}}
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
        EventPayload::PermissionDeny { ref tool_call_id, ref tool, .. }
            if tool_call_id == "toolu_cmd" && tool == "run_command"
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref status, ref model_content, .. }
            if status == "blocked"
                && model_content.contains("run_command was not executed because the permission gate denied this tool call")
    )));
}
