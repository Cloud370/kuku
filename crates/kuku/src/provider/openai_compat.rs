use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

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
    let has_tools = !request.assembly.tools.is_empty();
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
            Role::Assistant => messages.push(convert_assistant_message(message, has_tools)),
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
    if has_tools {
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

fn convert_assistant_message(message: &CanonicalMessage, has_tools: bool) -> Value {
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
    if let Some(replay) = native_replay(message, ProviderFormat::OpenAiChat) {
        if let Some(details) = replay.get("reasoning_details") {
            msg["reasoning_details"] = details.clone();
        } else if has_tools {
            if let Some(content) = replay.get("reasoning_content") {
                msg["reasoning_content"] = content.clone();
            }
        }
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
    tool_calls: BTreeMap<u64, (String, String)>,
    invalid_tool_stream: bool,
    saw_done: bool,
    stop_reason: Option<ModelStopReason>,
    reasoning_content: Option<String>,
    reasoning_details: Vec<Value>,
    replay_budget: ReplayBudget,
    reports: ReportAccumulator,
}

impl OpenAiCompatSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            started: false,
            tool_calls: BTreeMap::new(),
            invalid_tool_stream: false,
            saw_done: false,
            stop_reason: None,
            reasoning_content: None,
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
            if let Some(usage) = self.reports.usage(ProviderFormat::OpenAiChat, true) {
                self.chunks.push(usage);
            }
        }

        let choices = match data.get("choices").and_then(Value::as_array) {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(self.take_chunks()),
        };

        let choice = &choices[0];
        let delta = choice.get("delta").unwrap_or(&Value::Null);
        let reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .and_then(ModelStopReason::from_wire);
        let repeated_finish = reason.is_some() && reason == self.stop_reason;

        self.capture_reasoning(delta, repeated_finish)?;
        for field in ["content", "refusal"] {
            if let Some(text) = delta.get(field).and_then(nonempty_string) {
                self.chunks.push(ProviderChunk::TextDelta {
                    text: text.to_string(),
                });
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tool_calls {
                self.tool_delta(tc)?;
            }
        }

        if let Some(reason) = reason.filter(|_| !repeated_finish) {
            if self.stop_reason.replace(reason.clone()).is_none() {
                self.finish_reasoning()?;
                for &index in self.tool_calls.keys() {
                    self.chunks.push(ProviderChunk::ToolCallStop { index });
                }
            }
            self.chunks.push(ProviderChunk::StopReason {
                reason: if self.invalid_tool_stream {
                    ModelStopReason::InvalidResponse
                } else {
                    reason
                },
            });
        }

        Ok(self.take_chunks())
    }

    fn tool_delta(&mut self, delta: &Value) -> Result<(), ProviderFailure> {
        let Some(index) = delta.get("index").and_then(Value::as_u64) else {
            self.invalidate_tool_stream();
            return Ok(());
        };
        self.replay_budget.add(delta)?;
        let id = delta.get("id").and_then(nonempty_string);
        let name = delta.pointer("/function/name").and_then(nonempty_string);
        match self.tool_calls.get(&index) {
            Some((previous_id, previous_name)) => {
                if id.is_some_and(|id| id != previous_id)
                    || name.is_some_and(|name| name != previous_name)
                {
                    self.invalidate_tool_stream();
                }
            }
            None => {
                if self.tool_calls.len() >= MAX_REPLAY_ITEMS {
                    return Err(super::prepare::invalid_request(
                        "too many native tool calls",
                    ));
                }
                if let Some((id, name)) = id.zip(name).filter(|(id, name)| {
                    !id.trim().is_empty()
                        && !name.trim().is_empty()
                        && !self.tool_calls.values().any(|(existing, _)| existing == id)
                }) {
                    self.tool_calls
                        .insert(index, (id.to_string(), name.to_string()));
                    self.chunks.push(ProviderChunk::ToolCallStart {
                        index,
                        id: id.to_string(),
                        name: name.to_string(),
                    });
                } else {
                    self.invalidate_tool_stream();
                }
            }
        }
        if let Some(fragment) = delta
            .pointer("/function/arguments")
            .and_then(nonempty_string)
        {
            self.chunks.push(ProviderChunk::ToolCallArgDelta {
                index,
                fragment: fragment.to_string(),
            });
        }
        Ok(())
    }

    fn invalidate_tool_stream(&mut self) {
        self.invalid_tool_stream = true;
        self.chunks.push(ProviderChunk::InvalidToolStream);
    }

    fn capture_reasoning(
        &mut self,
        delta: &Value,
        repeated_finish: bool,
    ) -> Result<(), ProviderFailure> {
        let content = delta.get("reasoning_content").and_then(Value::as_str);
        let details = delta
            .get("reasoning_details")
            .and_then(Value::as_array)
            .filter(|_| !repeated_finish);
        if content.is_some_and(|content| !content.is_empty())
            || delta.get("reasoning").and_then(nonempty_string).is_some()
            || details.is_some_and(|items| !items.is_empty())
        {
            self.chunks.push(ProviderChunk::Reasoning);
        }
        if self.stop_reason.is_none() {
            if let Some(content) = content {
                self.replay_budget
                    .add(&Value::String(content.to_string()))?;
                self.reasoning_content
                    .get_or_insert_default()
                    .push_str(content);
            }
            if let Some(details) = details {
                self.merge_reasoning_details(details)?;
            }
        }
        Ok(())
    }

    fn finish_reasoning(&mut self) -> Result<(), ProviderFailure> {
        let value = if !self.reasoning_details.is_empty() {
            if self
                .reasoning_details
                .iter()
                .all(|item| reasoning_index(item).is_some())
            {
                self.reasoning_details.sort_by_key(reasoning_index);
            }
            json!({"reasoning_details": std::mem::take(&mut self.reasoning_details)})
        } else if let Some(content) = self.reasoning_content.take() {
            json!({"reasoning_content": content})
        } else {
            return Ok(());
        };
        validate_replay_size(&value)?;
        self.chunks.push(ProviderChunk::Replay {
            protocol: ProviderFormat::OpenAiChat,
            value,
        });
        Ok(())
    }

    fn merge_reasoning_details(&mut self, details: &[Value]) -> Result<(), ProviderFailure> {
        for detail in details {
            self.replay_budget.add(detail)?;
            let existing = self.reasoning_details.iter_mut().find(|value| {
                match reasoning_index(detail).zip(reasoning_index(value)) {
                    Some((incoming, prior)) => incoming == prior,
                    None => {
                        detail.get("id").and_then(nonempty_string).is_some()
                            && detail.get("id") == value.get("id")
                            && detail.get("type") == value.get("type")
                    }
                }
            });
            if let Some(existing) = existing {
                if ["id", "type"].iter().any(|field| {
                    existing
                        .get(field)
                        .and_then(nonempty_string)
                        .zip(detail.get(field).and_then(nonempty_string))
                        .is_some_and(|(prior, incoming)| prior != incoming)
                }) {
                    return Err(super::prepare::invalid_request(
                        "provider changed native reasoning item identity",
                    ));
                }
                if let (Some(target), Some(update)) = (existing.as_object_mut(), detail.as_object())
                {
                    for (key, value) in update {
                        let previous = target.get_mut(key);
                        if matches!(key.as_str(), "text" | "summary" | "data") {
                            if let (Some(Value::String(existing)), Some(fragment)) =
                                (previous, value.as_str())
                            {
                                existing.push_str(fragment);
                                continue;
                            }
                        } else if value.is_null() && previous.is_some() {
                            continue;
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

fn nonempty_string(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.is_empty())
}

fn reasoning_index(value: &Value) -> Option<u64> {
    value.get("index").and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_finish_reason_preserves_usage_without_repeating_replay_or_completion() {
        let mut parser = OpenAiCompatSseParser::new();
        parser.feed(concat!(
            "data: ",
            r#"{"id":"chat_gateway","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_read","function":{"name":"read_file","arguments":"{}"}}],"reasoning_details":[{"index":0,"type":"reasoning.encrypted","data":"cipher"}]},"finish_reason":null}]}"#
        )).unwrap();
        let terminal = parser
            .feed(r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        assert_eq!(
            1,
            terminal
                .iter()
                .filter(|chunk| matches!(chunk, ProviderChunk::Replay { .. }))
                .count()
        );
        assert_eq!(
            1,
            terminal
                .iter()
                .filter(|chunk| matches!(chunk, ProviderChunk::ToolCallStop { index: 0 }))
                .count()
        );
        assert_eq!(
            1,
            terminal
                .iter()
                .filter(|chunk| matches!(
                    chunk,
                    ProviderChunk::StopReason {
                        reason: ModelStopReason::ToolUse
                    }
                ))
                .count()
        );
        let duplicate = parser.feed(r#"data: {"choices":[{"delta":{"reasoning_details":[{"index":0,"type":"reasoning.encrypted","data":"cipher"}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"cost":0.001}}"#).unwrap();
        assert!(duplicate.iter().any(|chunk| matches!(chunk, ProviderChunk::Report { report } if report.cost_usd == Some(0.001))));
        assert!(duplicate.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StreamUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                ..
            }
        )));
        assert!(!duplicate.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::Replay { .. }
                | ProviderChunk::ToolCallStop { .. }
                | ProviderChunk::StopReason { .. }
        )));
    }

    #[test]
    fn conflicting_finish_reason_remains_visible_to_the_runtime() {
        let mut parser = OpenAiCompatSseParser::new();
        let mut chunks = parser
            .feed(r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        chunks.extend(
            parser
                .feed(r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#)
                .unwrap(),
        );
        let reasons = chunks
            .into_iter()
            .filter_map(|chunk| match chunk {
                ProviderChunk::StopReason { reason } => Some(reason),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            vec![ModelStopReason::ToolUse, ModelStopReason::EndTurn],
            reasons
        );
    }

    #[test]
    fn repeated_finish_reason_does_not_hide_late_tool_arguments() {
        let mut parser = OpenAiCompatSseParser::new();
        parser
            .feed(r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        let chunks = parser.feed(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::ToolCallArgDelta { index: 0, fragment } if fragment == "{}")));
        assert!(!chunks
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::StopReason { .. })));
    }

    #[test]
    fn parallel_tool_deltas_cannot_change_an_existing_index_or_reuse_its_id() {
        for conflicting in [
            r#"{"index":0,"id":"right","function":{"arguments":"{}"}}"#,
            r#"{"index":2,"id":"right","function":{"name":"read_file","arguments":"{}"}}"#,
        ] {
            let mut parser = OpenAiCompatSseParser::new();
            parser.feed(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"left","function":{"name":"read_file","arguments":"{}"}},{"index":1,"id":"right","function":{"name":"read_file","arguments":"{}"}}]}}]}"#).unwrap();
            let choices = json!([{"delta":{"tool_calls":[serde_json::from_str::<Value>(conflicting).unwrap()]},"finish_reason":"tool_calls"}]);
            let chunks = parser
                .feed(&format!("data: {}", json!({"choices":choices})))
                .unwrap();
            assert!(chunks
                .iter()
                .any(|chunk| matches!(chunk, ProviderChunk::InvalidToolStream)));
            assert!(chunks.iter().any(|chunk| matches!(
                chunk,
                ProviderChunk::StopReason {
                    reason: ModelStopReason::InvalidResponse
                }
            )));
        }
    }

    #[test]
    fn native_reasoning_keeps_explicit_order_and_signatures_through_fragmentation() {
        let mut parser = OpenAiCompatSseParser::new();
        for details in [
            json!([
                {"type":"reasoning.text","index":2,"id":"signed","text":"native","signature":null},
                {"type":"reasoning.encrypted","index":0,"id":"encrypted","data":"cipher"}
            ]),
            json!([{"type":"reasoning.text","id":"signed","text":" fragment","signature":"fixture-signature"}]),
            json!([{"type":"reasoning.text","index":2,"id":"signed","signature":null}]),
        ] {
            let chunks = parser
                .feed(&format!(
                    "data: {}",
                    json!({"choices":[{"delta":{"reasoning_details":details}}]})
                ))
                .unwrap();
            assert!(chunks
                .iter()
                .any(|chunk| matches!(chunk, ProviderChunk::Reasoning)));
            assert!(!chunks.iter().any(|chunk| matches!(
                chunk,
                ProviderChunk::ThinkingDelta { .. } | ProviderChunk::TextDelta { .. }
            )));
        }
        let chunks = parser
            .feed(r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#)
            .unwrap();
        let replay = chunks
            .iter()
            .find_map(|chunk| match chunk {
                ProviderChunk::Replay {
                    protocol: ProviderFormat::OpenAiChat,
                    value,
                } => Some(value),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            json!([
                {"type":"reasoning.encrypted","index":0,"id":"encrypted","data":"cipher"},
                {"type":"reasoning.text","index":2,"id":"signed","text":"native fragment","signature":"fixture-signature"}
            ]),
            replay["reasoning_details"]
        );
    }

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
