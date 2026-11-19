use super::{
    make_cancelled_run, make_test_pending, EventPayload, EventStore, PendingStep, Run, RunState,
    StreamingChunkState, UiEvent,
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
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
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
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
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
        model_permit: None,
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_1".to_string(),
        stream: Box::pin(tokio_stream::empty()),
        provider_attempt: 1,
        stream_failure: None,
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        model_phase: None,
        provider_replay: Vec::new(),
        stop_reason: Some(crate::event::ModelStopReason::EndTurn),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: None,
        request_timing: None,
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

    let state = StreamingChunkState {
        model_permit: None,
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_7".to_string(),
        stream: Box::pin(tokio_stream::empty()),
        provider_attempt: 1,
        stream_failure: None,
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        model_phase: None,
        provider_replay: Vec::new(),
        stop_reason: Some(crate::event::ModelStopReason::EndTurn),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: Some(crate::provider::types::ProviderUsage {
            input_tokens: Some(120),
            output_tokens: Some(30),
            cache_read_input_tokens: Some(900),
            cache_creation_input_tokens: Some(0),
        }),
        request_timing: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let step = crate::query::step::finish_streaming(state).await;

    assert!(matches!(step, Ok(PendingStep::Done(output, _, 1)) if output.text == "complete"));
    let log = std::fs::read_to_string(&log_path).expect("runtime log should be written");
    assert!(log.contains("\"kind\":\"runtime.model_usage\""));
    assert!(log.contains("\"request_id\":\"req_7\""));
    assert!(log.contains("\"cache_read_input_tokens\":900"));
    assert!(log.contains("\"cache_hit_rate\":"));
}

#[tokio::test]
async fn usage_overflow_persists_attempt_and_interrupts_turn() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut pending = make_test_pending(
        events_path.clone(),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.cumulative.output_tokens = Some(u64::MAX);
    let state = StreamingChunkState {
        model_permit: None,
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_2".to_string(),
        stream: Box::pin(tokio_stream::empty()),
        provider_attempt: 1,
        stream_failure: None,
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        model_phase: None,
        provider_replay: Vec::new(),
        stop_reason: Some(crate::event::ModelStopReason::EndTurn),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: Some(crate::provider::types::ProviderUsage {
            input_tokens: None,
            output_tokens: Some(1),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        }),
        request_timing: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let step = crate::query::step::finish_streaming(state).await.unwrap();

    assert!(matches!(
        step,
        PendingStep::Failed(crate::error::Error::InvalidEventStream(ref message))
            if message == "model usage overflow"
    ));
    let events = EventStore::replay(&events_path).unwrap();
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ModelResponse { .. }))
            .count()
    );
    assert_eq!(
        1,
        events
            .iter()
            .filter(|event| matches!(
                &event.payload,
                EventPayload::TurnInterrupted { reason, .. } if reason == "usage_overflow"
            ))
            .count()
    );
}

#[tokio::test]
async fn incomplete_handoff_marker_does_not_leak_to_final_output() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut pending = make_test_pending(
        events_path.clone(),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.handoff_triggered = true;

    let stream: std::pin::Pin<
        Box<
            dyn futures_core::Stream<
                    Item = std::result::Result<
                        crate::provider::chunk::ProviderChunk,
                        crate::provider::types::ProviderFailure,
                    >,
                > + Send,
        >,
    > = Box::pin(tokio_stream::iter(vec![
        Ok(crate::provider::chunk::ProviderChunk::TextDelta {
            text: "visible".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::TextDelta {
            text: "\n\n<kuku_handoff".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::StopReason {
            reason: crate::event::ModelStopReason::EndTurn,
        }),
        Ok(crate::provider::chunk::ProviderChunk::StreamEnd),
    ]));

    let mut streaming = StreamingChunkState {
        model_permit: None,
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_1".to_string(),
        stream,
        provider_attempt: 1,
        stream_failure: None,
        accumulated_text: String::new(),
        accumulated_thinking: String::new(),
        model_phase: None,
        provider_replay: Vec::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: None,
        request_timing: None,
        lead_events: Vec::new(),
        handoff_detector: Some(crate::query::handoff::HandoffDetector::new()),
        thinking_start: None,
        thinking_duration_ms: 0,
    };
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

    loop {
        match Run::poll_stream_chunk(&cancel_token, &mut streaming)
            .await
            .unwrap()
        {
            Some(UiEvent::TextDelta { text }) => assert_eq!(text, "visible"),
            Some(_) => continue,
            None => break,
        }
    }
    let step = crate::query::step::finish_streaming(streaming)
        .await
        .unwrap();

    let PendingStep::Done(output, _, _) = step else {
        panic!("expected done step");
    };
    assert_eq!(output.text, "visible");

    let events = EventStore::replay(&events_path).unwrap();
    let response = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ModelResponse { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .expect("model.response event");
    assert_eq!(response, "visible");
}

#[tokio::test]
async fn cancel_during_streaming_aborts_stream() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

    let token_clone = cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        token_clone.notify_waiters();
    });

    let pending = make_test_pending(events_path.clone(), dir.path(), cancel_token.clone());

    let stream: std::pin::Pin<
        Box<
            dyn futures_core::Stream<
                    Item = std::result::Result<
                        crate::provider::chunk::ProviderChunk,
                        crate::provider::types::ProviderFailure,
                    >,
                > + Send
                + Sync,
        >,
    > = Box::pin(tokio_stream::pending());

    let mut streaming = StreamingChunkState {
        model_permit: None,
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_1".to_string(),
        stream,
        provider_attempt: 1,
        stream_failure: None,
        accumulated_text: "partial".to_string(),
        accumulated_thinking: String::new(),
        model_phase: None,
        provider_replay: Vec::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: None,
        request_timing: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let _run = Run {
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
        session_id: "test".to_string(),
        state: RunState::Pending(Box::new(make_test_pending(
            events_path.clone(),
            dir.path(),
            cancel_token.clone(),
        ))),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: cancel_token.clone(),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    let result = Run::poll_stream_chunk(&cancel_token, &mut streaming)
        .await
        .unwrap();
    assert!(result.is_none());
    assert_eq!(
        Some(crate::event::ModelStopReason::Unknown(
            "cancelled".to_string()
        )),
        streaming.stop_reason
    );
}
