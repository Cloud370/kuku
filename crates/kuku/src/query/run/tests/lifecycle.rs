use super::{make_cancelled_run, make_test_pending, test_execution_scope, test_request_scope};
use crate::event::{EventPayload, EventStore};
use crate::query::types::{
    ExecSlot, PendingStep, Run, RunState, SlotEvent, StreamingChunkState, ToolKind, UiEvent,
};

#[tokio::test]
async fn cancel_when_idle_produces_turn_end() {
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
                execution: crate::event::test_execution_scope(),
                turn: 1,
                ts: "2026-05-20T00:00:00Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
    }

    let mut run = make_cancelled_run(events_path.clone(), 1);
    let result = run.next().await.unwrap();
    assert!(matches!(result, Some(UiEvent::Cancelled { turn: 1 })));
    let result = run.next().await.unwrap();
    assert!(result.is_none());

    let events = EventStore::replay(&events_path).unwrap();
    let last = events.last().unwrap();
    assert!(matches!(
        &last.payload,
        EventPayload::TurnCancelled { turn: 1, .. }
    ));
}

#[tokio::test]
async fn cancel_sets_token_and_transitions_state() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let run = Run {
        execution_scope: test_execution_scope(),
        session_id: "test".to_string(),
        state: RunState::Cancelled {
            events_path: events_path.clone(),
            turn: 1,
        },
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: cancel_token.clone(),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    cancel_token.notify_waiters();
    assert!(matches!(&run.state, RunState::Cancelled { .. }));
}

#[test]
fn runtime_log_emit_fans_out_before_best_effort_persistence_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut pending = make_test_pending(
        dir.path().join("events.jsonl"),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer =
        crate::log::BufferedLogWriter::with_flush_every(dir.path().join("runtime.jsonl"), 1);
    pending.runtime_log_writer.set_fail_after_bytes(Some(0));

    let result = crate::query::provider::emit_runtime_log(
        &mut pending,
        crate::log::LogLevel::Info,
        "runtime.test",
        "test log",
        None,
    );

    assert!(result.is_ok());
    let Some(UiEvent::Log { record }) = pending.pending_events.pop_front() else {
        panic!("expected host-visible log event before persistence failure");
    };
    assert_eq!(record.kind, "runtime.test");
}

#[tokio::test]
async fn runtime_log_persists_only_after_host_consumes_log_event() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let log_path = dir.path().join("runtime.jsonl");
    let mut pending = make_test_pending(
        events_path,
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer = crate::log::BufferedLogWriter::with_flush_every(&log_path, 1);
    crate::query::provider::emit_runtime_log(
        &mut pending,
        crate::log::LogLevel::Warn,
        "runtime.warn",
        "warn log",
        None,
    )
    .unwrap();

    assert!(
        !log_path.exists(),
        "disk write happened before host delivery"
    );

    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let mut run = Run {
        execution_scope: test_execution_scope(),
        session_id: "test".to_string(),
        state: RunState::Pending(Box::new(pending)),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    let event = run.next().await.unwrap().expect("log event");
    assert!(matches!(event, UiEvent::Log { .. }));
    assert!(
        !log_path.exists(),
        "disk write happened before host consumed log"
    );

    let _ = run.next().await;
    assert!(
        log_path.exists(),
        "disk write should happen after host consumption"
    );
}

#[tokio::test]
async fn completion_flush_failure_does_not_block_done() {
    let dir = tempfile::tempdir().unwrap();
    let mut pending = make_test_pending(
        dir.path().join("events.jsonl"),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer =
        crate::log::BufferedLogWriter::with_flush_every(dir.path().join("runtime.jsonl"), 64);
    pending.runtime_log_writer.set_fail_after_bytes(Some(0));
    crate::query::provider::emit_runtime_log(
        &mut pending,
        crate::log::LogLevel::Info,
        "runtime.test",
        "test log",
        None,
    )
    .unwrap();

    let state = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request: test_request_scope(),
        request_started: std::time::Instant::now(),
        stream: Box::pin(tokio_stream::empty()),
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: Some("end_turn".to_string()),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let step = crate::query::step::finish_streaming(state).await;

    assert!(matches!(step, Ok(PendingStep::Done(output, _, 1)) if output.text == "complete"));
}

#[tokio::test]
async fn completion_persists_runtime_model_usage_log() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let log_path = dir.path().join("runtime.jsonl");
    let mut pending = make_test_pending(
        events_path,
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer = crate::log::BufferedLogWriter::with_flush_every(&log_path, 1);

    let request = test_request_scope();
    let request_id = request.request_id.to_string();
    let state = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request,
        request_started: std::time::Instant::now(),
        stream: Box::pin(tokio_stream::empty()),
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: Some("end_turn".to_string()),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: Some(crate::provider::types::ProviderUsage {
            input_tokens: Some(120),
            output_tokens: Some(30),
            cache_read_input_tokens: Some(900),
            cache_creation_input_tokens: Some(0),
        }),
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let step = crate::query::step::finish_streaming(state).await;

    assert!(matches!(step, Ok(PendingStep::Done(output, _, 1)) if output.text == "complete"));
    let log = std::fs::read_to_string(&log_path).expect("runtime log should be written");
    assert!(log.contains("\"kind\":\"runtime.model_usage\""));
    assert!(log.contains(&format!("\"request_id\":\"{request_id}\"")));
    assert!(log.contains("\"cache_read_input_tokens\":900"));
    assert!(log.contains("\"cache_hit_rate\":"));
}

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
async fn cancelled_run_persists_tool_result_for_finished_active_slot() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::TurnStarted {
            execution: crate::event::test_execution_scope(),
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
            request: crate::event::test_request_scope("req_1".to_string()),
            index: 0,
            tool: "run_command".to_string(),
            args: serde_json::json!({"command": "printf hi", "timeout": 60, "brief": "print hi"}),
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
            ordered_with_simple_tools: false,
            label: "print hi".to_string(),
            cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
            nested_permissions: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        },
    );
    let mut run = Run {
        execution_scope: test_execution_scope(),
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
            SlotEvent::Done {
                status: "ok".to_string(),
                summary: "finished after cancellation".to_string(),
                model_content: String::new(),
                result: Some(serde_json::json!({"kind": "command_result"})),
            },
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
            EventPayload::ToolResult {
    tool_call_id, status, summary, .. }
                if tool_call_id == "tool_cancelled"
                    && status == "ok"
                    && summary == "finished after cancellation"
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
                execution: crate::event::test_execution_scope(),
                turn: 1,
                ts: "2026-05-20T00:00:00Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
        store
            .append(EventPayload::MessageUser {
                execution: crate::event::test_execution_scope(),
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
                turn: 1,
                ts: "2026-05-20T00:00:02Z".to_string(),
                request: crate::event::test_request_scope("req_1".to_string()),
                text: "partial".to_string(),
                thinking: None,
                input_tokens_total: None,
            })
            .unwrap();
        store
            .append(EventPayload::TurnCompleted {
                execution: crate::event::test_execution_scope(),
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
