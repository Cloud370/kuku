use super::{
    assert_rejected_tool_response, finish_tool_stream, make_test_pending, tool_stream_state,
    EventPayload, EventStore, PendingStep, Run,
};

#[tokio::test]
async fn native_replay_waits_for_the_session_to_accept_model_completion() {
    use crate::provider::chunk::ProviderChunk;
    use crate::query::UiEvent;

    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let store = EventStore::open(&events_path).unwrap();
    drop(store);
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut pending = make_test_pending(events_path.clone(), dir.path(), cancel.clone());
    pending.query.runtime_execution = Some(3);
    let mut streaming = tool_stream_state(
        pending,
        vec![
            ProviderChunk::Replay {
                protocol: crate::config::ProviderFormat::Anthropic,
                value: serde_json::json!({"native": "opaque provider state"}),
            },
            ProviderChunk::TextDelta {
                text: "visible".into(),
            },
        ],
    );
    assert!(
        matches!(Run::poll_stream_chunk(&cancel, &mut streaming).await.unwrap(),
        Some(UiEvent::TextDelta { text }) if text == "visible")
    );
    assert_eq!(1, streaming.provider_replay.len());
    drop(streaming);
    assert!(EventStore::replay(&events_path).unwrap().is_empty());
}

#[tokio::test]
async fn length_truncated_tool_call_persists_length_without_tool_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_truncated".to_string(),
                name: "run_command".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{\"command\":".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::Length,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::Length);
}

#[tokio::test]
async fn malformed_tool_json_is_classified_after_model_response_persists() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_bad_json".to_string(),
                name: "run_command".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{\"command\":".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
}

#[tokio::test]
async fn duplicate_terminal_fact_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
}

#[tokio::test]
async fn duplicate_tool_ids_and_indices_are_rejected_before_calls_are_persisted() {
    let cases = [
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_duplicate".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 1,
                id: "tool_duplicate".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 1 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_first".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_second".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    ];

    for chunks in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(events_path.clone(), dir.path(), chunks).await;

        assert!(matches!(step, Ok(PendingStep::Failed(_))));
        assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
    }
}

#[tokio::test]
async fn orphaned_deltas_and_missing_completions_are_rejected_before_calls_are_persisted() {
    let cases = [
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 1,
                id: "tool_valid_start".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{}".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 1 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_unfinished".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    ];

    for chunks in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(events_path.clone(), dir.path(), chunks).await;

        assert!(matches!(step, Ok(PendingStep::Failed(_))));
        assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
    }
}

#[tokio::test]
async fn missing_argument_buffers_are_rejected_before_calls_are_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
    let pending = make_test_pending(events_path.clone(), dir.path(), cancel_token.clone());
    let mut state = tool_stream_state(
        pending,
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_missing_buffer".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    );
    let poll = Run::poll_stream_chunk(&cancel_token, &mut state).await;
    assert!(matches!(
        poll,
        Ok(Some(crate::query::UiEvent::ModelProgress {
            phase: crate::query::ModelPhase::ToolArguments,
        }))
    ));
    let poll = Run::poll_stream_chunk(&cancel_token, &mut state).await;
    assert!(matches!(poll, Ok(None)));
    state.tool_arg_buffers.clear();

    let step = crate::query::step::finish_streaming(state).await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
}

#[tokio::test]
async fn empty_tool_names_and_non_object_arguments_are_rejected_before_calls_are_persisted() {
    let cases = [
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_without_name".to_string(),
                name: String::new(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_array_args".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "[]".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_scalar_args".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "42".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    ];

    for chunks in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(events_path.clone(), dir.path(), chunks).await;

        assert!(matches!(step, Ok(PendingStep::Failed(_))));
        assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
    }
}

#[tokio::test]
async fn end_turn_with_tool_calls_is_rejected_before_calls_are_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_end_turn".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
}

#[tokio::test]
async fn complete_tool_call_without_stop_reason_infers_tool_use() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_without_terminal".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await
    .unwrap();

    let PendingStep::Pending { pending, .. } = step else {
        panic!("expected complete legacy tool call to remain pending for execution");
    };
    assert_eq!(pending.queued_tool_calls.len(), 1);
    assert_eq!(
        pending.queued_tool_calls[0].tool_call.id,
        "tool_without_terminal"
    );
    assert!(EventStore::replay(&events_path)
        .unwrap()
        .iter()
        .any(|event| matches!(
            &event.payload,
            EventPayload::ModelResponse {
                stop_reason: Some(crate::event::ModelStopReason::ToolUse),
                ..
            }
        )));
}

#[tokio::test]
async fn incomplete_tool_lifecycle_without_stop_reason_is_rejected() {
    let cases = [
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_without_completion".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{}".to_string(),
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_with_invalid_args".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    ];

    for chunks in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(events_path.clone(), dir.path(), chunks).await;

        assert!(matches!(step, Ok(PendingStep::Failed(_))));
        assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
    }
}

#[tokio::test]
async fn explicit_non_success_wins_over_complete_tool_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_explicit_length".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::Length,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::Length);
}

#[tokio::test]
async fn later_stop_reason_cannot_replace_first_terminal_fact() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::Length,
            },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
}

#[tokio::test]
async fn length_then_tool_use_with_a_complete_call_never_queues_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::Length,
            },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_after_length".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{}".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
}

#[tokio::test]
async fn responses_done_arguments_reach_the_queued_tool_call() {
    let chunks = crate::provider::openai_responses::parse_responses_sse(concat!(
        "event: response.output_item.added\n",
        "data: {\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\"}}\n\n",
        "event: response.function_call_arguments.done\n",
        "data: {\"output_index\":0,\"item_id\":\"fc_1\",\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\",\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}\n\n",
        "event: response.output_item.done\n",
        "data: {\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\",\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}\n\n",
        "event: response.completed\n",
        "data: {\"response\":{\"status\":\"completed\",\"output\":[{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\",\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}]}}",
    ));
    let dir = tempfile::tempdir().unwrap();
    let step = finish_tool_stream(dir.path().join("events.jsonl"), dir.path(), chunks)
        .await
        .unwrap();

    let PendingStep::Pending { pending, .. } = step else {
        panic!("expected validated Responses call to remain pending for execution");
    };
    assert_eq!(pending.queued_tool_calls.len(), 1);
    assert_eq!(
        pending.queued_tool_calls[0].tool_call.args,
        serde_json::json!({"path": "a.txt"})
    );
}

#[tokio::test]
async fn tool_artifacts_after_terminal_fact_are_rejected() {
    let cases = [
        crate::provider::chunk::ProviderChunk::ToolCallStart {
            index: 0,
            id: "tool_after_terminal".to_string(),
            name: "read_file".to_string(),
        },
        crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
            index: 0,
            fragment: "{}".to_string(),
        },
        crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
    ];

    for artifact in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(
            events_path.clone(),
            dir.path(),
            vec![
                crate::provider::chunk::ProviderChunk::StopReason {
                    reason: crate::event::ModelStopReason::ToolUse,
                },
                artifact,
                crate::provider::chunk::ProviderChunk::StreamEnd,
            ],
        )
        .await;

        assert!(matches!(step, Ok(PendingStep::Failed(_))));
        assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
    }
}

#[tokio::test]
async fn usage_after_single_terminal_fact_remains_valid() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path,
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamUsage {
                input_tokens: Some(2),
                output_tokens: Some(3),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await
    .unwrap();

    let PendingStep::Done(_, Some(usage), _) = step else {
        panic!("expected successful end turn with usage");
    };
    assert_eq!(usage.input_tokens, Some(2));
    assert_eq!(usage.output_tokens, Some(3));
}

#[tokio::test]
async fn content_after_stop_reason_is_rejected() {
    let cases = [
        crate::provider::chunk::ProviderChunk::TextDelta {
            text: "late text".to_string(),
        },
        crate::provider::chunk::ProviderChunk::ThinkingDelta {
            text: "late thinking".to_string(),
        },
        crate::provider::chunk::ProviderChunk::ContentBlockStop { index: 0 },
    ];

    for content in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(
            events_path.clone(),
            dir.path(),
            vec![
                crate::provider::chunk::ProviderChunk::StopReason {
                    reason: crate::event::ModelStopReason::EndTurn,
                },
                content,
                crate::provider::chunk::ProviderChunk::StreamEnd,
            ],
        )
        .await;

        assert!(matches!(step, Ok(PendingStep::Failed(_))));
        assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
    }
}

#[tokio::test]
async fn protocol_facts_after_stream_end_are_rejected() {
    let cases = [
        crate::provider::chunk::ProviderChunk::ToolCallStart {
            index: 0,
            id: "tool_after_end".to_string(),
            name: "read_file".to_string(),
        },
        crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
            index: 0,
            fragment: "{}".to_string(),
        },
        crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
        crate::provider::chunk::ProviderChunk::ContentBlockStop { index: 0 },
        crate::provider::chunk::ProviderChunk::TextDelta {
            text: "late text".to_string(),
        },
        crate::provider::chunk::ProviderChunk::ThinkingDelta {
            text: "late thinking".to_string(),
        },
        crate::provider::chunk::ProviderChunk::StopReason {
            reason: crate::event::ModelStopReason::EndTurn,
        },
        crate::provider::chunk::ProviderChunk::StreamStart {
            request_id: "late_request".to_string(),
        },
        crate::provider::chunk::ProviderChunk::StreamEnd,
    ];

    for fact in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(
            events_path.clone(),
            dir.path(),
            vec![
                crate::provider::chunk::ProviderChunk::StopReason {
                    reason: crate::event::ModelStopReason::EndTurn,
                },
                crate::provider::chunk::ProviderChunk::StreamEnd,
                fact,
            ],
        )
        .await;

        assert!(matches!(step, Ok(PendingStep::Failed(_))));
        assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
    }
}

#[tokio::test]
async fn invalid_tool_signal_after_stream_end_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
            crate::provider::chunk::ProviderChunk::InvalidToolStream,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
}

#[tokio::test]
async fn usage_after_stream_end_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
            crate::provider::chunk::ProviderChunk::StreamUsage {
                input_tokens: Some(2),
                output_tokens: Some(3),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
}

#[tokio::test]
async fn end_turn_with_orphan_tool_delta_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{}".to_string(),
            },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::InvalidResponse);
}

#[tokio::test]
async fn duplicate_completion_for_single_tool_call_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_duplicate_completion".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
}

#[tokio::test]
async fn tool_completion_without_start_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
}

#[tokio::test]
async fn tool_delta_after_completion_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_delta_after_completion".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{}".to_string(),
            },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
}

#[tokio::test]
async fn whitespace_tool_call_id_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path.clone(),
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "  \t".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await;

    assert!(matches!(step, Ok(PendingStep::Failed(_))));
    assert_rejected_tool_response(&events_path, crate::event::ModelStopReason::ToolUse);
}

#[tokio::test]
async fn ordinary_content_block_stop_is_not_a_tool_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let step = finish_tool_stream(
        events_path,
        dir.path(),
        vec![
            crate::provider::chunk::ProviderChunk::ContentBlockStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::EndTurn,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    )
    .await
    .unwrap();

    assert!(matches!(step, PendingStep::Done(output, _, _) if output.text.is_empty()));
}

#[tokio::test]
async fn empty_and_literal_object_tool_buffers_are_valid_empty_objects() {
    let cases = [
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_empty_buffer".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
        vec![
            crate::provider::chunk::ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_literal_object".to_string(),
                name: "read_file".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{}".to_string(),
            },
            crate::provider::chunk::ProviderChunk::ToolCallStop { index: 0 },
            crate::provider::chunk::ProviderChunk::StopReason {
                reason: crate::event::ModelStopReason::ToolUse,
            },
            crate::provider::chunk::ProviderChunk::StreamEnd,
        ],
    ];

    for chunks in cases {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let step = finish_tool_stream(events_path.clone(), dir.path(), chunks)
            .await
            .unwrap();

        let PendingStep::Pending { pending, .. } = step else {
            panic!("expected complete tool call to remain pending for execution");
        };
        assert_eq!(pending.queued_tool_calls.len(), 1);
        assert_eq!(
            pending.queued_tool_calls[0].tool_call.args,
            serde_json::json!({})
        );
        assert!(EventStore::replay(&events_path)
            .unwrap()
            .iter()
            .any(|event| matches!(
                &event.payload,
                EventPayload::ToolCall { args, .. } if args == &serde_json::json!({})
            )));
    }
}
