use super::{
    assert_blocked_tool_result, make_queued_run, make_skill_queued_run, make_waiting_run,
    write_blocking_pre_hook,
};
use crate::event::{EventPayload, EventStore};
use crate::query::types::{
    PendingPermission, PermissionChoice, PermissionRequest, RunState, UiEvent,
};

#[tokio::test]
async fn queued_deny_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(
        dir.path().join("policy.md"),
        "# policy\n\n## allow\n\n## deny\n- run_command(printf hi)\n",
    )
    .unwrap();
    let mut run = make_queued_run(events_path.clone(), dir.path());

    let event = run.next().await.unwrap();

    assert!(
        matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
    );
    assert_blocked_tool_result(&events_path, "permission denied");
}

#[tokio::test]
async fn queued_pre_hook_block_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(
        dir.path().join("policy.md"),
        "# policy\n\n## allow\n- run_command(printf hi)\n\n## deny\n",
    )
    .unwrap();
    let pkg_dir = dir.path().join(".kuku").join("packages").join("test-hook");
    write_blocking_pre_hook(&pkg_dir, "blocked by hook");
    let mut run = make_queued_run(events_path.clone(), dir.path());
    if let RunState::Pending(pending) = &mut run.state {
        pending.plugin_registry = Some(std::sync::Arc::new(
            crate::plugin::PluginRegistry::builder()
                .load_packages(dir.path(), dir.path())
                .unwrap()
                .build()
                .unwrap(),
        ));
    }

    let event = run.next().await.unwrap();

    assert!(
        matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
    );
    assert_blocked_tool_result(&events_path, "blocked by hook");
}

#[tokio::test]
async fn inline_skill_tools_do_not_bypass_resolved_registry_membership() {
    let dir = tempfile::tempdir().unwrap();
    let registry = vec![crate::tool::ToolDefinition {
        name: "run_command".to_string(),
        description: "test command".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        read_only: false,
        max_result_chars: 8000,
        risk: "command".to_string(),
    }];

    for tool_name in ["use_skill", "list_skills", "search_skills"] {
        let events_path = dir.path().join(format!("{tool_name}.jsonl"));
        std::fs::write(&events_path, "").unwrap();
        let mut run = make_skill_queued_run(events_path, dir.path(), registry.clone(), tool_name);

        let event = run.next().await.unwrap();

        assert!(
            matches!(event, Some(UiEvent::Error { code, message }) if code == "unknown_tool" && message == format!("unknown tool: {tool_name}"))
        );
    }
}

#[tokio::test]
async fn decide_pre_hook_block_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(dir.path().join("policy.md"), "# policy\n").unwrap();
    let pkg_dir = dir.path().join(".kuku").join("packages").join("test-hook");
    write_blocking_pre_hook(&pkg_dir, "blocked after allow");
    let mut run = make_queued_run(events_path.clone(), dir.path());
    let waiting = match std::mem::replace(&mut run.state, RunState::Done(None)) {
        RunState::Pending(mut pending) => {
            pending.plugin_registry = Some(std::sync::Arc::new(
                crate::plugin::PluginRegistry::builder()
                    .load_packages(dir.path(), dir.path())
                    .unwrap()
                    .build()
                    .unwrap(),
            ));
            PendingPermission {
                request: PermissionRequest {
                    id: "tool_queued".to_string(),
                    conversation: crate::conversation::address::ConversationAddress::MAIN,
                    turn: 1,
                    tool_call_id: "tool_queued".to_string(),
                    tool: "run_command".to_string(),
                    risk: "command".to_string(),
                    summary: "print hi".to_string(),
                    candidate: "printf hi".to_string(),
                    source: "default_ask".to_string(),
                },
                pending: *pending,
            }
        }
        other => panic!("expected pending run, got {other:?}"),
    };
    run.state = RunState::WaitingForPermission(Box::new(waiting));

    let event = run
        .decide("tool_queued", PermissionChoice::Once, None)
        .await
        .unwrap();

    assert!(
        matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
    );
    assert_blocked_tool_result(&events_path, "blocked after allow");
}

#[test]
fn cancel_pending_permission_rejects_mismatched_queued_tool() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = make_waiting_run(
        dir.path().join("events.jsonl"),
        dir.path(),
        "req_cancel",
        "tool_request",
        "tool_queued",
    );

    let error = run.cancel_pending_permission("req_cancel").unwrap_err();

    assert!(
        matches!(error, crate::error::Error::InvalidEventStream(message) if message.contains("tool_request") && message.contains("tool_queued"))
    );
    assert!(matches!(
        &run.state,
        RunState::WaitingForPermission(waiting)
            if waiting.request.tool_call_id == "tool_request"
                && waiting.pending.queued_tool_calls.front().unwrap().tool_call.id == "tool_queued"
    ));
}

#[test]
fn cancel_pending_permission_restores_state_when_persistence_fails() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events_dir");
    std::fs::create_dir(&events_path).unwrap();
    let mut run = make_waiting_run(
        events_path,
        dir.path(),
        "req_cancel",
        "tool_cancel",
        "tool_cancel",
    );

    let error = run.cancel_pending_permission("req_cancel").unwrap_err();

    assert!(matches!(error, crate::error::Error::Io(_)));
    assert!(matches!(
        &run.state,
        RunState::WaitingForPermission(waiting)
            if waiting.request.id == "req_cancel"
                && waiting.pending.queued_tool_calls.front().unwrap().tool_call.id == "tool_cancel"
    ));
}

#[tokio::test]
async fn cancel_waiting_permission_writes_cancelled_result_without_deny() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut run = make_waiting_run(
        events_path.clone(),
        dir.path(),
        "req_cancel",
        "tool_cancel",
        "tool_cancel",
    );

    run.cancel();
    let event = run.next().await.unwrap();

    assert!(matches!(event, Some(UiEvent::Cancelled { turn: 1 })));
    let events = EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref tool_call_id, ref status, ref structured, .. }
            if tool_call_id == "tool_cancel"
                && status == "cancelled"
                && structured == &Some(serde_json::json!({"kind": "cancelled"}))
    )));
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionDeny { ref tool_call_id, .. } if tool_call_id == "tool_cancel"
    )));
}
