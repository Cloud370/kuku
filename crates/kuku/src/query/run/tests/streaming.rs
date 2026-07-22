use super::{make_test_pending, test_execution_scope, test_request_scope};
use crate::event::{EventPayload, EventStore};
use crate::query::types::{PendingStep, Run, RunState, StreamingChunkState, UiEvent};

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
            reason: "end_turn".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::StreamEnd),
    ]));

    let mut streaming = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request: test_request_scope(),
        request_started: std::time::Instant::now(),
        stream,
        accumulated_text: String::new(),
        accumulated_thinking: String::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
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
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request: test_request_scope(),
        request_started: std::time::Instant::now(),
        stream,
        accumulated_text: "partial".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let _run = Run {
        execution_scope: test_execution_scope(),
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
    assert_eq!(streaming.stop_reason.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn cancelled_next_call_preserves_streaming_state() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
    let pending = make_test_pending(events_path, dir.path(), cancel_token.clone());
    let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(8);
    chunk_tx
        .send(Ok(crate::provider::chunk::ProviderChunk::TextDelta {
            text: "Hi!".to_string(),
        }))
        .await
        .unwrap();

    let streaming = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request: test_request_scope(),
        request_started: std::time::Instant::now(),
        stream: Box::pin(tokio_stream::wrappers::ReceiverStream::new(chunk_rx)),
        accumulated_text: String::new(),
        accumulated_thinking: String::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let mut run = Run {
        execution_scope: test_execution_scope(),
        session_id: "test".to_string(),
        state: RunState::Streaming(Box::new(streaming)),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token,
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::TextDelta { text }) if text == "Hi!"
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), run.next())
            .await
            .is_err()
    );

    chunk_tx
        .send(Ok(crate::provider::chunk::ProviderChunk::StopReason {
            reason: "end_turn".to_string(),
        }))
        .await
        .expect("cancelled next call must not drop the provider stream");
    chunk_tx
        .send(Ok(crate::provider::chunk::ProviderChunk::StreamEnd))
        .await
        .unwrap();
    drop(chunk_tx);

    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::Done { output, .. }) if output.text == "Hi!"
    ));
}

#[tokio::test]
async fn malformed_tool_call_arguments_fail_instead_of_staying_empty_object() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

    let pending = make_test_pending(events_path, dir.path(), cancel_token.clone());
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
        Ok(crate::provider::chunk::ProviderChunk::ToolCallStart {
            index: 0,
            id: "tool_bad_args".to_string(),
            name: "run_command".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
            index: 0,
            fragment: "{\"command\":".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::ContentBlockStop { index: 0 }),
        Ok(crate::provider::chunk::ProviderChunk::StopReason {
            reason: "tool_use".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::StreamEnd),
    ]));

    let mut streaming = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request: test_request_scope(),
        request_started: std::time::Instant::now(),
        stream,
        accumulated_text: String::new(),
        accumulated_thinking: String::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let error = loop {
        match Run::poll_stream_chunk(&cancel_token, &mut streaming).await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("expected malformed tool args to fail"),
            Err(error) => break error,
        }
    };

    assert!(matches!(
        error,
        crate::error::Error::Provider { kind: crate::provider::types::ProviderFailureKind::InvalidRequest, message, .. }
            if message.contains("tool_bad_args")
    ));
}
