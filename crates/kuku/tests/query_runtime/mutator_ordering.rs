use std::future::Future as _;
use std::task::Poll;

use super::*;

async fn wait_for_file(path: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("command did not reach the read barrier");
}

fn tool_end_status(event: UiEvent, expected_id: &str) -> (String, Option<serde_json::Value>) {
    match event {
        UiEvent::ToolEnd {
            id, status, result, ..
        } => {
            assert_eq!(id, expected_id);
            (status, result)
        }
        other => panic!("expected ToolEnd for {expected_id}, got {other:?}"),
    }
}

async fn next_tool_start(run: &mut Run, tool_call_id: &str) -> UiEvent {
    loop {
        let event = run.next().await.unwrap().expect("event");
        if matches!(&event, UiEvent::ToolStart { id, .. } if id == tool_call_id) {
            return event;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn command_mutation_finishes_before_same_run_edit_starts() {
    let env = TestEnv::new();
    let target = env.workspace.path().join("race.txt");
    std::fs::write(&target, "alpha\n").unwrap();

    let snapshot_server = MockServer::start();
    snapshot_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("1\\talpha");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_snapshot_done",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Snapshot ready."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 8, "output_tokens": 4}
            })));
    });
    snapshot_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("prepare race snapshot");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_snapshot",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Reading the target."},
                    {"type": "tool_use", "id": "toolu_snapshot", "name": "read_file", "input": {"path": "race.txt"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let session_id = "s_same_run_mutator_ordering";
    let snapshot = query("prepare race snapshot")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(snapshot_server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();
    assert_eq!(snapshot.text, "Snapshot ready.");

    let race_server = MockServer::start();
    race_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("run conflicting mutations");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_race",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Mutating the target."},
                    {"type": "tool_use", "id": "toolu_command", "name": "run_command", "input": {
                        "command": "saved=$(cat race.txt); printf ready > command-read; while [ ! -f release-command ]; do sleep 0.01; done; printf '%s\\ncommand\\n' \"$saved\" > race.txt",
                        "timeout": 10,
                        "brief": "write from a captured copy"
                    }},
                    {"type": "tool_use", "id": "toolu_edit", "name": "edit_file", "input": {
                        "path": "race.txt",
                        "old_text": "alpha",
                        "new_text": "edited",
                        "brief": "edit the captured line"
                    }}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 8, "output_tokens": 12}
            })));
    });

    let mut run = query("run conflicting mutations")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(race_server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let command_request = next_permission_request(&mut run).await;
    assert_eq!(command_request.tool_call_id, "toolu_command");
    let command_start = run
        .decide(&command_request.id, PermissionChoice::Once, None)
        .await
        .unwrap();
    assert!(
        matches!(command_start, Some(UiEvent::ToolStart { ref id, .. }) if id == "toolu_command")
    );
    wait_for_file(&env.workspace.path().join("command-read")).await;

    let mut next_event = Box::pin(run.next());
    let early_edit_request = std::future::poll_fn(|cx| {
        Poll::Ready(match next_event.as_mut().poll(cx) {
            Poll::Ready(event) => Some(event),
            Poll::Pending => None,
        })
    })
    .await;
    drop(next_event);

    let edit_end = if let Some(event) = early_edit_request {
        let edit_start = event.unwrap().expect("queued edit event");
        assert!(matches!(edit_start, UiEvent::ToolStart { ref id, .. } if id == "toolu_edit"));
        let edit_end = next_tool_end(&mut run, "toolu_edit").await;
        std::fs::write(env.workspace.path().join("release-command"), "go").unwrap();
        let _ = next_tool_end(&mut run, "toolu_command").await;
        edit_end
    } else {
        std::fs::write(env.workspace.path().join("release-command"), "go").unwrap();
        let _ = next_tool_end(&mut run, "toolu_command").await;
        let _ = next_tool_start(&mut run, "toolu_edit").await;
        next_tool_end(&mut run, "toolu_edit").await
    };

    let (status, result) = tool_end_status(edit_end, "toolu_edit");
    assert_eq!(status, "error");
    assert_eq!(
        result.unwrap()["reason_code"],
        serde_json::Value::String("snapshot_stale".to_string())
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "alpha\ncommand\n");
}

#[tokio::test(flavor = "current_thread")]
async fn command_mutation_finishes_before_same_run_memory_write_starts() {
    let env = TestEnv::new();
    let workspace = std::fs::canonicalize(env.workspace.path()).expect("canonical workspace path");
    let memory_path = kuku::session::project_memory_path(env.home.path(), &workspace)
        .expect("project memory path");
    std::fs::create_dir_all(memory_path.parent().unwrap()).unwrap();
    std::fs::write(
        &memory_path,
        "# memory\n\n## how_to_work\n- existing rule\n\n## what_is_true\n\n## where_to_look\n",
    )
    .unwrap();
    let command = format!(
        "cp '{}' captured-memory; printf ready > memory-read; while [ ! -f release-memory-command ]; do sleep 0.01; done; cp captured-memory '{}'",
        memory_path.display(),
        memory_path.display()
    );

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("run memory conflict");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_memory_race",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "toolu_memory_command", "name": "run_command", "input": {
                        "command": command,
                        "timeout": 10,
                        "brief": "restore captured memory"
                    }},
                    {"type": "tool_use", "id": "toolu_remember", "name": "remember_memory", "input": {
                        "scope": "project",
                        "kind": "what_is_true",
                        "text": "serialized memory update"
                    }}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 8, "output_tokens": 12}
            })));
    });

    let mut run = query("run memory conflict")
        .session("s_same_run_memory_ordering")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let command_request = next_permission_request(&mut run).await;
    assert_eq!(command_request.tool_call_id, "toolu_memory_command");
    let command_start = run
        .decide(&command_request.id, PermissionChoice::Once, None)
        .await
        .unwrap();
    assert!(
        matches!(command_start, Some(UiEvent::ToolStart { ref id, .. }) if id == "toolu_memory_command")
    );
    wait_for_file(&env.workspace.path().join("memory-read")).await;

    let mut next_event = Box::pin(run.next());
    let early_memory_start = std::future::poll_fn(|cx| {
        Poll::Ready(match next_event.as_mut().poll(cx) {
            Poll::Ready(event) => Some(event),
            Poll::Pending => None,
        })
    })
    .await;
    drop(next_event);

    let memory_end = if let Some(event) = early_memory_start {
        let memory_start = event.unwrap().expect("queued memory event");
        assert!(
            matches!(memory_start, UiEvent::ToolStart { ref id, .. } if id == "toolu_remember")
        );
        let memory_end = next_tool_end(&mut run, "toolu_remember").await;
        std::fs::write(env.workspace.path().join("release-memory-command"), "go").unwrap();
        let _ = next_tool_end(&mut run, "toolu_memory_command").await;
        memory_end
    } else {
        std::fs::write(env.workspace.path().join("release-memory-command"), "go").unwrap();
        let _ = next_tool_end(&mut run, "toolu_memory_command").await;
        let _ = next_tool_start(&mut run, "toolu_remember").await;
        next_tool_end(&mut run, "toolu_remember").await
    };

    let (status, _) = tool_end_status(memory_end, "toolu_remember");
    assert_eq!(status, "ok");
    let final_memory = std::fs::read_to_string(memory_path).unwrap();
    assert!(
        final_memory.contains("- serialized memory update"),
        "same-run command overwrote the memory update: {final_memory}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn read_only_tool_starts_while_ordered_tool_waits_for_active_mutation() {
    let env = TestEnv::new();
    std::fs::write(env.workspace.path().join("queued-read.txt"), "visible\n").unwrap();

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("run mixed concurrency");
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_mixed_concurrency",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "toolu_blocking_command", "name": "run_command", "input": {
                        "command": "printf ready > mixed-command-ready; while [ ! -f release-mixed-command ]; do sleep 0.01; done",
                        "timeout": 10,
                        "brief": "wait for release"
                    }},
                    {"type": "tool_use", "id": "toolu_ordered_read", "name": "read_file", "input": {
                        "path": "queued-read.txt"
                    }},
                    {"type": "tool_use", "id": "toolu_independent_find", "name": "find_files", "input": {
                        "pattern": "*.txt"
                    }}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 8, "output_tokens": 12}
            })));
    });

    let mut run = query("run mixed concurrency")
        .session("s_mixed_same_run_concurrency")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .start()
        .await
        .unwrap();

    let command_request = next_permission_request(&mut run).await;
    assert_eq!(command_request.tool_call_id, "toolu_blocking_command");
    let command_start = run
        .decide(&command_request.id, PermissionChoice::Once, None)
        .await
        .unwrap();
    assert!(
        matches!(command_start, Some(UiEvent::ToolStart { ref id, .. }) if id == "toolu_blocking_command")
    );
    wait_for_file(&env.workspace.path().join("mixed-command-ready")).await;

    let mut next_event = Box::pin(run.next());
    let early_event = std::future::poll_fn(|cx| {
        Poll::Ready(match next_event.as_mut().poll(cx) {
            Poll::Ready(event) => Some(event),
            Poll::Pending => None,
        })
    })
    .await;
    drop(next_event);
    std::fs::write(env.workspace.path().join("release-mixed-command"), "go").unwrap();

    let event = early_event.expect("independent read-only tool was blocked behind ordered work");
    assert!(matches!(
        event.unwrap().expect("read-only tool event"),
        UiEvent::ToolStart { ref id, .. } if id == "toolu_independent_find"
    ));
    let _ = next_tool_end(&mut run, "toolu_independent_find").await;
}
