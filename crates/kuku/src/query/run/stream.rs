use super::{
    append_model_error, append_turn_interrupted, Error, ProviderChunk, ProviderToolCall, Result,
    Run, StreamingChunkState, UiEvent,
};

impl Run {
    pub(super) async fn poll_stream_chunk(
        cancel_token: &tokio::sync::Notify,
        streaming: &mut StreamingChunkState,
    ) -> Result<Option<UiEvent>> {
        use tokio_stream::StreamExt;
        loop {
            let chunk = tokio::select! {
                chunk = streaming.stream.next() => match chunk {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(failure)) => {
                        return Err(crate::error::Error::Provider {
                            kind: failure.kind,
                            message: failure.message,
                            provider: None,
                            model: None,
                        });
                    }
                    None => return Ok(None),
                },
                _ = cancel_token.notified() => {
                    streaming.stop_reason = Some("cancelled".to_string());
                    return Ok(None);
                }
            };

            match chunk {
                ProviderChunk::StreamStart { request_id: rid } => {
                    streaming.provider_request_id = Some(rid);
                }
                ProviderChunk::TextDelta { text } => {
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    if let Some(ref mut detector) = streaming.handoff_detector {
                        if let Some(user_text) = detector.process(&text) {
                            if !user_text.is_empty() {
                                streaming.accumulated_text.push_str(&user_text);
                                return Ok(Some(UiEvent::TextDelta { text: user_text }));
                            }
                        }
                        return Ok(None);
                    }
                    streaming.accumulated_text.push_str(&text);
                    return Ok(Some(UiEvent::TextDelta { text }));
                }
                ProviderChunk::ThinkingDelta { text } => {
                    if streaming.thinking_start.is_none() {
                        streaming.thinking_start = Some(std::time::Instant::now());
                    }
                    streaming.accumulated_thinking.push_str(&text);
                    return Ok(Some(UiEvent::ThinkingDelta { text }));
                }
                ProviderChunk::ToolCallStart { index, id, name } => {
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    streaming.tool_calls.push(ProviderToolCall {
                        id,
                        name,
                        args: serde_json::json!({}),
                        index,
                    });
                    streaming.tool_arg_buffers.push((index, String::new()));
                }
                ProviderChunk::ToolCallArgDelta { index, fragment } => {
                    if let Some((_, buf)) = streaming
                        .tool_arg_buffers
                        .iter_mut()
                        .find(|(i, _)| *i == index)
                    {
                        buf.push_str(&fragment);
                    }
                }
                ProviderChunk::ContentBlockStop { index } => {
                    if let Some((_, buf)) =
                        streaming.tool_arg_buffers.iter().find(|(i, _)| *i == index)
                    {
                        match serde_json::from_str::<serde_json::Value>(buf) {
                            Ok(args) => {
                                if let Some(tc) =
                                    streaming.tool_calls.iter_mut().find(|t| t.index == index)
                                {
                                    tc.args = args;
                                }
                            }
                            Err(error) => {
                                let tool_call_id = streaming
                                    .tool_calls
                                    .iter()
                                    .find(|t| t.index == index)
                                    .map(|tool_call| tool_call.id.clone())
                                    .unwrap_or_else(|| format!("index {index}"));
                                return Err(crate::error::Error::Provider {
                                    kind: crate::provider::types::ProviderFailureKind::InvalidRequest,
                                    message: format!(
                                        "tool call {tool_call_id} has invalid JSON arguments: {error}"
                                    ),
                                    provider: None,
                                    model: None,
                                });
                            }
                        }
                    }
                }
                ProviderChunk::StopReason { reason } => {
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    streaming.stop_reason = Some(reason);
                }
                ProviderChunk::StreamUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                } => {
                    let entry =
                        streaming
                            .usage
                            .get_or_insert(crate::provider::types::ProviderUsage {
                                input_tokens: Some(0),
                                output_tokens: Some(0),
                                cache_read_input_tokens: Some(0),
                                cache_creation_input_tokens: Some(0),
                            });
                    entry.input_tokens = Some(entry.input_tokens.unwrap_or(0) + input_tokens);
                    entry.output_tokens = Some(entry.output_tokens.unwrap_or(0) + output_tokens);
                    entry.cache_read_input_tokens =
                        Some(entry.cache_read_input_tokens.unwrap_or(0) + cache_read_input_tokens);
                    entry.cache_creation_input_tokens = Some(
                        entry.cache_creation_input_tokens.unwrap_or(0)
                            + cache_creation_input_tokens,
                    );
                }
                ProviderChunk::ServerError { code, message } => {
                    return Err(crate::error::Error::Provider {
                        kind: crate::provider::types::ProviderFailureKind::Unknown,
                        message: format!("{code}: {message}"),
                        provider: None,
                        model: None,
                    });
                }
                ProviderChunk::StreamEnd => {}
            }
        }
    }
}

pub(super) fn record_streaming_provider_error_facts(
    streaming: &StreamingChunkState,
    error: &Error,
) {
    let Error::Provider { kind, message, .. } = error else {
        return;
    };
    let _ = append_model_error(
        &streaming.pending.events_path,
        streaming.request.clone(),
        streaming.pending.turn,
        provider_failure_event_kind(*kind),
        message,
    );
    let _ = append_turn_interrupted(
        &streaming.pending.events_path,
        streaming.pending.execution_scope(),
        &streaming.conversation,
        streaming.pending.turn,
        provider_failure_event_kind(*kind),
    );
}

fn provider_failure_event_kind(kind: crate::provider::types::ProviderFailureKind) -> &'static str {
    match kind {
        crate::provider::types::ProviderFailureKind::Authentication => "authentication",
        crate::provider::types::ProviderFailureKind::RateLimited => "rate_limited",
        crate::provider::types::ProviderFailureKind::ContextTooLarge => "context_too_large",
        crate::provider::types::ProviderFailureKind::InvalidRequest => "invalid_request",
        crate::provider::types::ProviderFailureKind::ProviderUnavailable => "provider_unavailable",
        crate::provider::types::ProviderFailureKind::Transport => "transport",
        crate::provider::types::ProviderFailureKind::Internal => "internal",
        crate::provider::types::ProviderFailureKind::Unknown => "unknown",
    }
}
