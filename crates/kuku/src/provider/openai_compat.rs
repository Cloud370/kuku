use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

use crate::config::ProviderFormat;
use crate::context::{CanonicalMessage, MessageBlock, Role};
use crate::event::ModelStopReason;

use super::chunk::ProviderChunk;
use super::replay::{native_replay, validate_replay_size, ReplayBudget, MAX_REPLAY_ITEMS};
use super::report::ReportAccumulator;
use super::sse::{parse_sse_frame, stream_sse_events};
use super::trace::ProviderTraceDirection;
use super::transport::{send_prepared, TransportResponse};
use super::types::{PreparedProviderRequest, ProviderFailure, ProviderRequest, ResolvedProvider};

const TRUNCATED_STREAM_MESSAGE: &str = "provider stream ended before [DONE]";

pub(crate) fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

pub(crate) fn render_body(request: &ProviderRequest<'_>) -> Value {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.assembly.system_prompt,
    })];
    for message in &request.assembly.prelude_messages {
        messages.extend(convert_user_message(message));
    }
    if let Some(summary) = &request.assembly.handoff_summary {
        let template = &request.catalog.runtime["handoff-context"].text;
        let content = template.replace("{{handoff_summary}}", summary);
        messages.push(json!({
            "role": "user",
            "content": content,
        }));
    }
    for message in &request.assembly.history {
        match message.role {
            Role::User => messages.extend(convert_user_message(message)),
            Role::Assistant => messages.push(convert_assistant_message(message)),
        }
    }

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": false,
    });

    if let Some(max_tokens) = request.max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if request.think_level != crate::config::ThinkLevel::Off {
        let effort = match request.think_level.as_str() {
            "low" => request
                .thinking
                .low
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("low"),
            "medium" => request
                .thinking
                .medium
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("medium"),
            "high" => request
                .thinking
                .high
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("xhigh"),
            _ => "medium",
        };
        body["reasoning_effort"] = json!(effort);
    }
    if !request.assembly.tools.is_empty() {
        body["tools"] = json!(request
            .assembly
            .tools
            .iter()
            .map(|schema| {
                json!({
                    "type": "function",
                    "function": {
                        "name": schema.name,
                        "description": schema.description,
                        "parameters": schema.input_schema,
                    }
                })
            })
            .collect::<Vec<_>>());
    }

    body
}

fn convert_user_message(message: &CanonicalMessage) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut text_parts = Vec::new();

    for block in &message.blocks {
        match block {
            MessageBlock::Text(text) => text_parts.push(text.clone()),
            MessageBlock::ToolResult(result) => {
                if !text_parts.is_empty() {
                    messages.push(json!({"role": "user", "content": text_parts.join("\n\n")}));
                    text_parts.clear();
                }
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": result.tool_call_id,
                    "content": result.model_content,
                }));
            }
            MessageBlock::ToolUse(_)
            | MessageBlock::Thinking(_)
            | MessageBlock::ProviderReplay { .. } => {}
        }
    }

    if !text_parts.is_empty() {
        messages.push(json!({"role": "user", "content": text_parts.join("\n\n")}));
    }

    messages
}

fn convert_assistant_message(message: &CanonicalMessage) -> Value {
    let thinking = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Thinking(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    let text = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(text) => Some(text.as_str()),
            MessageBlock::ToolUse(_)
            | MessageBlock::ToolResult(_)
            | MessageBlock::Thinking(_)
            | MessageBlock::ProviderReplay { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("");

    let tool_calls = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::ToolUse(tool_use) => Some(json!({
                "id": tool_use.id,
                "type": "function",
                "function": {
                    "name": tool_use.name,
                    "arguments": serde_json::to_string(&tool_use.args)
                        .unwrap_or_else(|_| "{}".to_string()),
                }
            })),
            MessageBlock::Text(_)
            | MessageBlock::ToolResult(_)
            | MessageBlock::Thinking(_)
            | MessageBlock::ProviderReplay { .. } => None,
        })
        .collect::<Vec<_>>();

    let mut msg = json!({"role": "assistant"});
    if let Some(details) = native_replay(message, ProviderFormat::OpenAiChat)
        .and_then(|value| value.get("reasoning_details"))
    {
        msg["reasoning_details"] = details.clone();
    }
    if !thinking.is_empty() && msg.get("reasoning_details").is_none() {
        msg["reasoning_content"] = json!(thinking);
    }
    if tool_calls.is_empty() {
        msg["content"] = json!(text);
    } else {
        if !text.is_empty() {
            msg["content"] = json!(text);
        }
        msg["tool_calls"] = json!(tool_calls);
    }
    msg
}

pub(crate) async fn stream(
    config: &ResolvedProvider,
    request: &ProviderRequest<'_>,
    trace_metadata: Option<super::trace::ProviderTraceMetadata>,
) -> Result<super::ProviderChunkStream, ProviderFailure> {
    let prepared = super::prepare_provider_request(config, request, None)?;
    super::stream_prepared_provider(config, &prepared, trace_metadata).await
}

pub(crate) async fn stream_prepared(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    trace_metadata: Option<super::trace::ProviderTraceMetadata>,
) -> Result<super::ProviderChunkStream, ProviderFailure> {
    let TransportResponse { response, trace } =
        send_prepared(config, prepared, trace_metadata).await?;
    let url = prepared.url.clone();
    let parser = Arc::new(Mutex::new(OpenAiCompatSseParser::new()));
    let frame_parser = Arc::clone(&parser);
    let eof_parser = Arc::clone(&parser);
    let frame_trace = trace.clone();
    Ok(stream_sse_events(
        response,
        move |frame| {
            if let Some(trace) = &frame_trace {
                trace.record(
                    ProviderTraceDirection::Event,
                    Some(&url),
                    None,
                    json!({ "frame": frame }),
                );
            }
        },
        move |frame| frame_parser.lock().unwrap().feed(frame),
        move || eof_parser.lock().unwrap().finish(),
    ))
}

struct OpenAiCompatSseParser {
    chunks: Vec<ProviderChunk>,
    started: bool,
    tool_call_indices: Vec<u64>,
    invalid_tool_stream: bool,
    saw_done: bool,
    reasoning_details: Vec<Value>,
    replay_budget: ReplayBudget,
    reports: ReportAccumulator,
}

impl OpenAiCompatSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            started: false,
            tool_call_indices: Vec::new(),
            invalid_tool_stream: false,
            saw_done: false,
            reasoning_details: Vec::new(),
            replay_budget: ReplayBudget::default(),
            reports: ReportAccumulator::default(),
        }
    }

    fn feed(&mut self, frame: &str) -> Result<Vec<ProviderChunk>, ProviderFailure> {
        if frame.is_empty() {
            return Ok(self.take_chunks());
        }

        let Some(frame) = parse_sse_frame(frame) else {
            return Ok(self.take_chunks());
        };
        let data_str = frame.data.trim();

        if data_str == "[DONE]" {
            self.saw_done = true;
            self.chunks.push(ProviderChunk::StreamEnd);
            return Ok(self.take_chunks());
        }

        let data: Value = serde_json::from_str(data_str).map_err(|_| {
            super::prepare::invalid_request("provider emitted malformed JSON event")
        })?;

        if let Some(err) = data.get("error") {
            let code = err
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("server_error")
                .to_string();
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("server error")
                .to_string();
            self.chunks
                .push(ProviderChunk::ServerError { code, message });
            return Ok(self.take_chunks());
        }

        if !self.started {
            let rid = data
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.chunks
                .push(ProviderChunk::StreamStart { request_id: rid });
            self.started = true;
        }

        if let Some(report) = self.reports.observe(&data)? {
            self.chunks.push(ProviderChunk::Report { report });
        }
        if data.get("usage").is_some_and(Value::is_object) {
            if let Some(usage) = self.reports.usage(ProviderFormat::OpenAiChat) {
                self.chunks.push(usage);
            }
        }

        let choices = match data.get("choices").and_then(Value::as_array) {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(self.take_chunks()),
        };

        let choice = &choices[0];
        let delta = choice.get("delta");

        if let Some(text) = delta.and_then(|d| d.get("content")).and_then(Value::as_str) {
            if !text.is_empty() {
                self.chunks.push(ProviderChunk::TextDelta {
                    text: text.to_string(),
                });
            }
        }

        if let Some(text) = delta
            .and_then(|d| d.get("reasoning_content").or_else(|| d.get("reasoning")))
            .and_then(Value::as_str)
        {
            if !text.is_empty() {
                self.chunks.push(ProviderChunk::ThinkingDelta {
                    text: text.to_string(),
                });
            }
        }

        if let Some(details) = delta
            .and_then(|delta| delta.get("reasoning_details"))
            .and_then(Value::as_array)
        {
            self.merge_reasoning_details(details)?;
        }

        if let Some(tool_calls) = delta
            .and_then(|d| d.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for tc in tool_calls {
                let Some(index) = tc.get("index").and_then(Value::as_u64) else {
                    self.invalid_tool_stream = true;
                    self.chunks.push(ProviderChunk::InvalidToolStream);
                    continue;
                };
                let function = tc.get("function");

                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    let name = function
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.chunks.push(ProviderChunk::ToolCallStart {
                        index,
                        id: id.to_string(),
                        name,
                    });
                    if !self.tool_call_indices.contains(&index) {
                        self.tool_call_indices.push(index);
                    }
                }

                if let Some(args) = function
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                {
                    if !args.is_empty() {
                        self.chunks.push(ProviderChunk::ToolCallArgDelta {
                            index,
                            fragment: args.to_string(),
                        });
                    }
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            if !self.reasoning_details.is_empty() {
                let value = json!({"reasoning_details": self.reasoning_details});
                validate_replay_size(&value)?;
                self.chunks.push(ProviderChunk::Replay {
                    protocol: ProviderFormat::OpenAiChat,
                    value,
                });
            }
            for &idx in &self.tool_call_indices {
                self.chunks.push(ProviderChunk::ToolCallStop { index: idx });
            }
            self.tool_call_indices.clear();

            if let Some(reason) = ModelStopReason::from_wire(reason) {
                self.chunks.push(ProviderChunk::StopReason {
                    reason: if self.invalid_tool_stream {
                        ModelStopReason::InvalidResponse
                    } else {
                        reason
                    },
                });
            }
        }

        Ok(self.take_chunks())
    }

    fn merge_reasoning_details(&mut self, details: &[Value]) -> Result<(), ProviderFailure> {
        for detail in details {
            self.replay_budget.add(detail)?;
            let existing = detail
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|index| {
                    self.reasoning_details
                        .iter_mut()
                        .find(|value| value.get("index").and_then(Value::as_u64) == Some(index))
                });
            if let Some(existing) = existing {
                if let (Some(target), Some(update)) = (existing.as_object_mut(), detail.as_object())
                {
                    for (key, value) in update {
                        if matches!(key.as_str(), "text" | "summary" | "data") {
                            if let (Some(existing), Some(fragment)) = (
                                target.get(key).and_then(Value::as_str).map(str::to_string),
                                value.as_str(),
                            ) {
                                target.insert(key.clone(), Value::String(existing + fragment));
                                continue;
                            }
                        }
                        target.insert(key.clone(), value.clone());
                    }
                }
            } else {
                if self.reasoning_details.len() >= MAX_REPLAY_ITEMS {
                    return Err(super::prepare::invalid_request(
                        "too many native reasoning items",
                    ));
                }
                self.reasoning_details.push(detail.clone());
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<Vec<ProviderChunk>, ProviderFailure> {
        if self.saw_done {
            return Ok(self.take_chunks());
        }

        Err(ProviderFailure {
            kind: crate::provider::types::ProviderFailureKind::Transport,
            message: TRUNCATED_STREAM_MESSAGE.to_string(),
            status: None,
            provider_request_id: None,
            retryable: true,
        })
    }

    fn take_chunks(&mut self) -> Vec<ProviderChunk> {
        std::mem::take(&mut self.chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_error_emits_server_error_chunk() {
        let mut parser = OpenAiCompatSseParser::new();
        let frame =
            r#"data: {"error": {"type": "server_error", "message": "something went wrong"}}"#;
        let chunks = parser.feed(frame).unwrap();
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            ProviderChunk::ServerError { code, message } => {
                assert_eq!(code, "server_error");
                assert_eq!(message, "something went wrong");
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[test]
    fn usage_on_completion_chunk_is_emitted() {
        let mut parser = OpenAiCompatSseParser::new();
        let frame = r#"data: {"id":"chatcmpl_usage","choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":88,"completion_tokens":18,"total_tokens":106,"prompt_tokens_details":{"cached_tokens":34}}}"#;

        let chunks = parser.feed(frame).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StreamUsage {
                input_tokens: Some(88),
                output_tokens: Some(18),
                cache_read_input_tokens: Some(34),
                cache_creation_input_tokens: None
            }
        )));
    }

    #[test]
    fn deepseek_usage_reports_prompt_cache_hits() {
        let mut parser = OpenAiCompatSseParser::new();
        let frame = r#"data: {"id":"chatcmpl_usage","choices":[],"usage":{"prompt_tokens":88,"completion_tokens":18,"prompt_cache_hit_tokens":34,"prompt_cache_miss_tokens":54}}"#;

        let chunks = parser.feed(frame).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StreamUsage {
                input_tokens: Some(88),
                output_tokens: Some(18),
                cache_read_input_tokens: Some(34),
                cache_creation_input_tokens: None
            }
        )));
    }

    #[test]
    fn tool_finish_reason_emits_typed_tool_completion() {
        let mut parser = OpenAiCompatSseParser::new();
        parser
            .feed(
                r#"data: {"id":"chatcmpl_tool","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":null}]}"#,
            )
            .unwrap();

        let chunks = parser
            .feed(
                r#"data: {"id":"chatcmpl_tool","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            )
            .unwrap();

        assert!(chunks
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::ToolCallStop { index: 0 })));
    }

    #[test]
    fn missing_or_malformed_tool_index_is_invalid() {
        for index in ["null", "\"zero\""] {
            let mut parser = OpenAiCompatSseParser::new();
            parser.feed(&format!(
                r#"data: {{"id":"chatcmpl_tool","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":{index},"id":"call_1","function":{{"name":"read_file","arguments":"{{}}"}}}}]}},"finish_reason":null}}]}}"#,
            )).unwrap();
            let chunks = parser.feed(
                r#"data: {"id":"chatcmpl_tool","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            ).unwrap();
            assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
        }

        let mut parser = OpenAiCompatSseParser::new();
        let chunks = parser.feed(
            r#"data: {"id":"chatcmpl_tool","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":null}]}"#,
        ).unwrap();
        assert!(chunks
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::ToolCallStart { index: 0, .. })));
    }

    #[test]
    fn truncated_stream_does_not_emit_stream_end() {
        let mut parser = OpenAiCompatSseParser::new();
        let mut chunks = parser
            .feed(
            r#"data: {"id":"chatcmpl_truncated","choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}"#,
        )
            .unwrap();
        chunks.extend(parser.feed("").unwrap());

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::TextDelta { text } if text == "partial"
        )));
        assert!(!chunks
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::StreamEnd)));
    }
}
