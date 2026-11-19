use super::{
    make_waiting_run, EventPayload, EventStore, ExecSlot, Run, RunState, SlotEvent, ToolKind,
    UiEvent,
};
use crate::tool::builtin::test_helpers::noisy_timeout_command;

#[tokio::test]
async fn cancelled_tool_result_envelope_has_correct_fields() {
    let result = crate::tool::ToolResultEnvelope::cancelled("test cancel");
    assert_eq!(result.status, "cancelled");
    assert_eq!(result.summary, "test cancel");
    assert!(result.model_content.is_empty());
    assert!(!result.truncated);
    assert_eq!(
        result.structured,
        Some(serde_json::json!({"kind": "cancelled"}))
    );
}

#[tokio::test]
async fn command_slot_preserves_timeout_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let _slot = crate::query::slots::spawn_command_slot(
        "tool_timeout".to_string(),
        None,
        serde_json::json!({
            "shell":if cfg!(windows) {"cmd"} else {"sh"},"cwd":".","command": noisy_timeout_command(dir.path()),
            "timeout": 1,
            "brief": "produce bounded timeout output",
        }),
        "produce bounded timeout output".to_string(),
        dir.path().to_path_buf(),
        event_tx,
        None,
        crate::WorkspaceExecutionHost::default(),
        None,
    );

    loop {
        let (_, event) = event_rx.recv().await.unwrap();
        if let SlotEvent::Done(result) = event {
            assert_eq!("error", result.status);
            assert!(result.truncated);
            break;
        }
    }
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

#[tokio::test]
async fn cancelled_run_persists_tool_result_for_finished_active_slot() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-05-20T00:00:00Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-05-20T00:00:01Z".to_string(),
            conversation: None,
            tool_call_id: "tool_cancelled".to_string(),
            request_id: "req_1".to_string(),
            index: 0,
            tool: "run_command".to_string(),
            args: serde_json::json!({"program": "printf", "args": ["hi"], "cwd": ".", "timeout": 60, "brief": "print hi"}),
        })
        .unwrap();

    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        "tool_cancelled".to_string(),
        ExecSlot {
            tool_call_id: "tool_cancelled".to_string(),
            conversation: None,
            kind: ToolKind::Command { pid: None },
            workspace_ordered: false,
            job: None,
            label: "print hi".to_string(),
            cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
            nested_permissions: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        },
    );
    let mut run = Run {
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
        session_id: "test".to_string(),
        state: RunState::Cancelled {
            events_path: events_path.clone(),
            turn: 1,
        },
        slots,
        slot_event_tx: slot_event_tx.clone(),
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    slot_event_tx
        .send((
            "tool_cancelled".to_string(),
            SlotEvent::Done(crate::tool::ToolResultEnvelope::ok_truncated(
                "finished after cancellation",
                "",
                serde_json::json!({"kind": "command_result"}),
            )),
        ))
        .await
        .unwrap();

    let event = run.next().await.unwrap();

    assert!(matches!(
        event,
        Some(UiEvent::ToolEnd { ref id, ref status, .. })
            if id == "tool_cancelled" && status == "ok"
    ));
    let events = EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { tool_call_id, status, summary, truncated, .. }
            if tool_call_id == "tool_cancelled"
                && status == "ok"
                && summary == "finished after cancellation"
                && *truncated
    )));
}

#[tokio::test]
async fn resume_after_cancel_includes_turn_end_in_history() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    {
        let mut store = EventStore::open(&events_path).unwrap();
        store
            .append(EventPayload::SessionCreated {
                ts: "2026-05-20T00:00:00Z".to_string(),
                schema_version: 2,
                session_id: "test".to_string(),
                created_at: "2026-05-20T00:00:00Z".to_string(),
                kuku_version: "0.1.0".to_string(),
            })
            .unwrap();
        store
            .append(EventPayload::TurnStarted {
                turn: 1,
                ts: "2026-05-20T00:00:00Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
        store
            .append(EventPayload::MessageUser {
                turn: 1,
                ts: "2026-05-20T00:00:01Z".to_string(),
                conversation: "main".to_string(),
                text: "hello".to_string(),
                from: None,
                via_tool_call_id: None,
            })
            .unwrap();
        store
            .append(EventPayload::ModelResponse {
                conversation: None,
                turn: 1,
                ts: "2026-05-20T00:00:02Z".to_string(),
                request_id: "req_1".to_string(),
                text: "partial".to_string(),
                thinking: None,
                stop_reason: None,
                input_tokens_total: None,
                output_tokens_total: None,
            })
            .unwrap();
        store
            .append(EventPayload::TurnCompleted {
                turn: 1,
                ts: "2026-05-20T00:00:03Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
    }

    let events = EventStore::replay(&events_path).unwrap();
    let (summary, history) = crate::context::rebuild_history(
        &events,
        &crate::conversation::address::ConversationAddress::MAIN,
    );
    assert!(summary.is_none());
    assert_eq!(history.len(), 2);
    let messages: Vec<_> = history.iter().map(|m| format!("{:?}", m.role)).collect();
    assert!(messages.contains(&"User".to_string()));
    assert!(messages.contains(&"Assistant".to_string()));
}
