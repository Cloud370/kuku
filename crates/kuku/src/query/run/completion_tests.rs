use super::*;
use crate::event::{EventPayload, EventStore};
use crate::query::types::{ExecSlot, ToolKind};

#[tokio::test]
async fn async_error_completion_is_counted_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut pending = tests::make_test_pending(
        events_path.clone(),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.record_tool_call("run_command");
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        "tool_error".to_string(),
        ExecSlot {
            tool_call_id: "tool_error".to_string(),
            conversation: None,
            kind: ToolKind::Command { pid: None },
            workspace_ordered: false,
            label: "failing command".to_string(),
            cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
            nested_permissions: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        },
    );
    let mut run = Run {
        session_id: "test".to_string(),
        state: RunState::Pending(Box::new(pending)),
        slots,
        slot_event_tx: slot_event_tx.clone(),
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };
    slot_event_tx
        .send((
            "tool_error".to_string(),
            SlotEvent::Done {
                status: "error".to_string(),
                summary: "command timed out".to_string(),
                model_content: "timeout output".to_string(),
                truncated: false,
                result: Some(serde_json::json!({"kind": "error"})),
            },
        ))
        .await
        .unwrap();

    let event = run.next().await.unwrap();
    assert!(matches!(
        event,
        Some(UiEvent::ToolEnd { status, result, .. })
            if status == "error" && result == Some(serde_json::json!({"kind": "error"}))
    ));
    let pending = match std::mem::replace(&mut run.state, RunState::Done(None)) {
        RunState::Pending(pending) => *pending,
        other => panic!("expected pending run, got {other:?}"),
    };
    let step = crate::query::step::finish_streaming(StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_complete".to_string(),
        stream: Box::pin(tokio_stream::empty()),
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: Some(crate::event::ModelStopReason::EndTurn),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    })
    .await
    .unwrap();
    let PendingStep::Done(output, _, _) = step else {
        panic!("expected completed run");
    };

    assert_eq!(1, output.tool_summary.total_calls);
    assert_eq!(1, output.tool_summary.errors);
    let events = EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { tool_call_id, status, structured, .. }
            if tool_call_id == "tool_error"
                && status == "error"
                && structured == &Some(serde_json::json!({"kind": "error"}))
    )));
}

#[tokio::test]
async fn length_stop_interrupts_without_completing_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let pending = tests::make_test_pending(
        events_path.clone(),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    let step = crate::query::step::finish_streaming(StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_length".to_string(),
        stream: Box::pin(tokio_stream::empty()),
        accumulated_text: "partial output".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: Some(crate::event::ModelStopReason::Length),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    })
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    let events = EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::TurnInterrupted { ref reason, .. } if reason == "length"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::TurnCompleted { .. })));
}
