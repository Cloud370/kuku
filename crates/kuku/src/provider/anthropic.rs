use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
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

const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 4096;

const TRUNCATED_STREAM_MESSAGE: &str = "provider stream ended before message_stop";

pub(crate) fn messages_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

pub(crate) fn render_body(request: &ProviderRequest<'_>) -> Value {
    let mut messages = request
        .assembly
        .prelude_messages
        .iter()
        .map(convert_canonical_message)
        .collect::<Vec<_>>();
    if let Some(summary) = &request.assembly.handoff_summary {
        let template = &request.catalog.runtime["handoff-context"].text;
        let content = template.replace("{{handoff_summary}}", summary);
        messages.push(json!({
            "role": "user",
            "content": content,
        }));
    }
    messages.extend(
        request
            .assembly
            .history
            .iter()
            .map(convert_canonical_message),
    );

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS as u32),
        "stream": false,
        "cache_control": cache_control_ephemeral(),
        "system": [{
            "type": "text",
            "text": request.assembly.system_prompt,
        }],
    });

    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if request.think_level != crate::config::ThinkLevel::Off {
        body["thinking"] = json!({
            "type": "adaptive",
            "display": "summarized",
        });
        let effort = match request.think_level.as_str() {
            "low" => "low",
            "medium" => "medium",
            "high" => "max",
            _ => "medium",
        };
        body["output_config"] = json!({ "effort": effort });
    }
    if !request.assembly.tools.is_empty() {
        body["tools"] = json!(request
            .assembly
            .tools
            .iter()
            .map(|schema| {
                json!({
                    "name": schema.name,
                    "description": schema.description,
                    "input_schema": schema.input_schema,
                })
            })
            .collect::<Vec<_>>());
    }
    body
}

fn cache_control_ephemeral() -> Value {
    json!({"type": "ephemeral"})
}

fn convert_canonical_message(message: &CanonicalMessage) -> Value {
    if message.role == Role::Assistant {
        if let Some(native) = native_replay(message, ProviderFormat::Anthropic) {
            if native.get("role").and_then(Value::as_str) == Some("assistant")
                && native.get("content").is_some_and(Value::is_array)
            {
                return native.clone();
            }
        }
    }
    match message.role {
        Role::User => {
            let content = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Text(text) => Some(json!({"type": "text", "text": text})),
                    MessageBlock::ToolResult(result) => Some(json!({
                        "type": "tool_result",
                        "tool_use_id": result.tool_call_id,
                        "content": result.model_content,
                        "is_error": result.status != "ok",
                    })),
                    MessageBlock::ToolUse(_)
                    | MessageBlock::Thinking(_)
                    | MessageBlock::ProviderReplay { .. } => None,
                })
                .collect::<Vec<_>>();

            json!({"role": "user", "content": content})
        }
        Role::Assistant => {
            let content = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Thinking(text) => {
                        Some(json!({"type": "thinking", "thinking": text}))
                    }
                    MessageBlock::Text(text) => Some(json!({"type": "text", "text": text})),
                    MessageBlock::ToolUse(tool_use) => Some(json!({
                        "type": "tool_use",
                        "id": tool_use.id,
                        "name": tool_use.name,
                        "input": tool_use.args,
                    })),
                    MessageBlock::ToolResult(_) | MessageBlock::ProviderReplay { .. } => None,
                })
                .collect::<Vec<_>>();

            json!({"role": "assistant", "content": content})
        }
    }
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
    let parser = Arc::new(Mutex::new(AnthropicSseParser::new()));
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

struct AnthropicSseParser {
    chunks: Vec<ProviderChunk>,
    tool_blocks: BTreeSet<u64>,
    invalid_tool_stream: bool,
    saw_terminal: bool,
    content_blocks: BTreeMap<u64, Value>,
    completed_blocks: BTreeSet<u64>,
    tool_arguments: BTreeMap<u64, String>,
    replay_budget: ReplayBudget,
    reports: ReportAccumulator,
}

impl AnthropicSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            tool_blocks: BTreeSet::new(),
            invalid_tool_stream: false,
            saw_terminal: false,
            content_blocks: BTreeMap::new(),
            completed_blocks: BTreeSet::new(),
            tool_arguments: BTreeMap::new(),
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
        if frame.data.is_empty() || frame.event.as_deref() == Some("ping") {
            return Ok(self.take_chunks());
        }
        let data: Value = serde_json::from_str(&frame.data).map_err(|_| {
            super::prepare::invalid_request("provider emitted malformed JSON event")
        })?;

        let event_type = frame
            .event
            .as_deref()
            .or_else(|| data.get("type").and_then(Value::as_str))
            .unwrap_or("");
        match event_type {
            "message_start" => {
                if let Some(msg) = data.get("message") {
                    let rid = msg
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.chunks
                        .push(ProviderChunk::StreamStart { request_id: rid });
                    self.record_report(msg, false)?;
                }
            }
            "content_block_start" => {
                if let Some(block) = data.get("content_block") {
                    self.replay_budget.add(block)?;
                    if self.content_blocks.len() >= MAX_REPLAY_ITEMS {
                        return Err(super::prepare::invalid_request(
                            "too many native content blocks",
                        ));
                    }
                    if let Some(index) = data.get("index").and_then(Value::as_u64) {
                        if self.content_blocks.insert(index, block.clone()).is_some() {
                            self.invalid_tool_stream = true;
                            self.chunks.push(ProviderChunk::InvalidToolStream);
                        }
                    }
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = block
                                .get("text")
                                .and_then(Value::as_str)
                                .filter(|text| !text.is_empty())
                            {
                                self.chunks
                                    .push(ProviderChunk::TextDelta { text: text.into() });
                            }
                        }
                        Some("thinking") => {
                            if let Some(text) = block
                                .get("thinking")
                                .and_then(Value::as_str)
                                .filter(|text| !text.is_empty())
                            {
                                self.chunks
                                    .push(ProviderChunk::ThinkingDelta { text: text.into() });
                            }
                        }
                        Some("tool_use") => {
                            let Some(index) = data.get("index").and_then(Value::as_u64) else {
                                self.invalid_tool_stream = true;
                                self.chunks.push(ProviderChunk::InvalidToolStream);
                                return Ok(self.take_chunks());
                            };
                            self.tool_blocks.insert(index);
                            let id = block
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.chunks.push(ProviderChunk::ToolCallStart {
                                index,
                                id: id.clone(),
                                name: name.clone(),
                            });
                            if let Some(input) = block
                                .get("input")
                                .and_then(Value::as_object)
                                .filter(|input| !input.is_empty())
                            {
                                self.chunks.push(ProviderChunk::ToolCallArgDelta {
                                    index,
                                    fragment: serde_json::to_string(input).expect("input is JSON"),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_delta" => {
                if let Some(delta) = data.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = delta
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.append_native(
                                data.get("index").and_then(Value::as_u64),
                                "text",
                                &text,
                            )?;
                            self.chunks.push(ProviderChunk::TextDelta { text });
                        }
                        Some("thinking_delta") => {
                            let text = delta
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.append_native(
                                data.get("index").and_then(Value::as_u64),
                                "thinking",
                                &text,
                            )?;
                            self.chunks.push(ProviderChunk::ThinkingDelta { text });
                        }
                        Some("signature_delta") => {
                            if let Some(signature) = delta.get("signature").and_then(Value::as_str)
                            {
                                self.append_native(
                                    data.get("index").and_then(Value::as_u64),
                                    "signature",
                                    signature,
                                )?;
                            }
                        }
                        Some("input_json_delta") => {
                            let Some(index) = data.get("index").and_then(Value::as_u64) else {
                                self.invalid_tool_stream = true;
                                self.chunks.push(ProviderChunk::InvalidToolStream);
                                return Ok(self.take_chunks());
                            };
                            let fragment = delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.replay_budget.add(&Value::String(fragment.clone()))?;
                            self.tool_arguments
                                .entry(index)
                                .or_default()
                                .push_str(&fragment);
                            if !self.tool_blocks.contains(&index)
                                || self
                                    .content_blocks
                                    .get(&index)
                                    .and_then(|block| block.get("input"))
                                    .and_then(Value::as_object)
                                    .is_some_and(|input| !input.is_empty())
                            {
                                self.invalid_tool_stream = true;
                                self.chunks.push(ProviderChunk::InvalidToolStream);
                            }
                            self.chunks
                                .push(ProviderChunk::ToolCallArgDelta { index, fragment });
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let Some(index) = data.get("index").and_then(Value::as_u64) else {
                    self.invalid_tool_stream = true;
                    self.chunks.push(ProviderChunk::InvalidToolStream);
                    return Ok(self.take_chunks());
                };
                if !self.completed_blocks.insert(index) {
                    self.invalid_tool_stream = true;
                    self.chunks.push(ProviderChunk::InvalidToolStream);
                }
                if self.tool_blocks.contains(&index) {
                    if let Some(arguments) = self.tool_arguments.remove(&index) {
                        match serde_json::from_str::<Value>(&arguments) {
                            Ok(input) if input.is_object() => {
                                if let Some(block) = self.content_blocks.get_mut(&index) {
                                    block["input"] = input;
                                }
                            }
                            Ok(_) | Err(_) => {
                                self.invalid_tool_stream = true;
                                self.chunks.push(ProviderChunk::InvalidToolStream);
                            }
                        }
                    } else if self
                        .content_blocks
                        .get(&index)
                        .and_then(|block| block.get("input"))
                        .and_then(Value::as_object)
                        .is_some_and(|input| input.is_empty())
                    {
                        self.chunks.push(ProviderChunk::ToolCallArgDelta {
                            index,
                            fragment: "{}".into(),
                        });
                    }
                    self.chunks.push(ProviderChunk::ToolCallStop { index });
                } else {
                    self.chunks.push(ProviderChunk::ContentBlockStop { index });
                }
            }
            "message_delta" => {
                if let Some(delta) = data.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
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
                }
                let output_reported = data
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                    .is_some();
                self.record_report(&data, output_reported)?;
            }
            "message_stop" => {
                if !self.invalid_tool_stream
                    && !self.content_blocks.is_empty()
                    && self
                        .content_blocks
                        .keys()
                        .all(|index| self.completed_blocks.contains(index))
                {
                    let value = json!({"role": "assistant", "content": self.content_blocks.values().cloned().collect::<Vec<_>>()});
                    validate_replay_size(&value)?;
                    self.chunks.push(ProviderChunk::Replay {
                        protocol: ProviderFormat::Anthropic,
                        value,
                    });
                }
                self.saw_terminal = true;
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "error" => {
                let code = data
                    .get("error")
                    .and_then(|e| e.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let message = data
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("server error")
                    .to_string();
                self.chunks
                    .push(ProviderChunk::ServerError { code, message });
            }
            _ => {}
        }

        Ok(self.take_chunks())
    }

    fn append_native(
        &mut self,
        index: Option<u64>,
        field: &str,
        fragment: &str,
    ) -> Result<(), ProviderFailure> {
        self.replay_budget.add(&Value::String(fragment.into()))?;
        if let Some(block) = index.and_then(|index| self.content_blocks.get_mut(&index)) {
            let existing = block.get(field).and_then(Value::as_str).unwrap_or("");
            block[field] = Value::String(existing.to_string() + fragment);
        }
        Ok(())
    }

    fn record_report(
        &mut self,
        response: &Value,
        output_reported: bool,
    ) -> Result<(), ProviderFailure> {
        if let Some(report) = self.reports.observe(response)? {
            self.chunks.push(ProviderChunk::Report { report });
        }
        if response.get("usage").is_some_and(Value::is_object) {
            if let Some(usage) = self
                .reports
                .usage(ProviderFormat::Anthropic, output_reported)
            {
                self.chunks.push(usage);
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<Vec<ProviderChunk>, ProviderFailure> {
        if self.saw_terminal {
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
    use crate::context::{CanonicalMessage, ContextAssembly};
    use crate::prompt::PromptCatalog;
    use crate::provider::types::ProviderRequest;

    fn minimal_request<'a>(
        think_level: crate::config::ThinkLevel,
        catalog: &'a PromptCatalog,
    ) -> ProviderRequest<'a> {
        ProviderRequest {
            assembly: ContextAssembly {
                system_prompt: "test".into(),
                prelude_messages: vec![],
                history: vec![],
                tools: vec![],
                prompt_asset_sources: vec![],
                project_instruction_sources: vec![],
                memory_sources: vec![],
                runtime_context: None,
                handoff_summary: None,
            },
            catalog,
            current_input: crate::provider::types::CanonicalPromptInput {
                parts: vec![CanonicalMessage::user_text("input")],
            },
            model: "test-model".into(),
            max_output_tokens: Some(1024),
            temperature: None,
            stream: false,
            think_level,
            thinking: crate::config::ResolvedThinking {
                low: None,
                medium: None,
                high: None,
            },
        }
    }

    #[test]
    fn render_body_adaptive_thinking_high() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::High, &catalog));
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["display"], "summarized");
        assert_eq!(body["output_config"]["effort"], "max");
        assert!(body.get("budget_tokens").is_none());
    }

    #[test]
    fn render_body_adaptive_thinking_medium() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(
            crate::config::ThinkLevel::Medium,
            &catalog,
        ));
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "medium");
    }

    #[test]
    fn render_body_adaptive_thinking_low() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::Low, &catalog));
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn render_body_no_thinking_when_off() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::Off, &catalog));
        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn render_body_includes_cache_control() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::Off, &catalog));
        assert_eq!(body["cache_control"]["type"], "ephemeral");
        assert!(body["system"][0].get("cache_control").is_none());
    }

    #[test]
    fn messages_initial_output_is_not_normalized_as_final_usage() {
        let mut parser = AnthropicSseParser::new();
        let start = parser.feed(
            "event: message_start\ndata: {\"message\":{\"id\":\"msg_use\",\"usage\":{\"input_tokens\":80,\"cache_read_input_tokens\":15,\"cache_creation_input_tokens\":5,\"output_tokens\":1}}}",
        ).unwrap();
        assert!(start.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StreamUsage {
                input_tokens: Some(100),
                output_tokens: None,
                cache_read_input_tokens: Some(15),
                ..
            }
        )));
        let delta = parser.feed(
            "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12}}",
        ).unwrap();
        assert!(delta.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StreamUsage {
                input_tokens: Some(100),
                output_tokens: Some(12),
                cache_read_input_tokens: Some(15),
                ..
            }
        )));
    }

    #[test]
    fn truncated_stream_does_not_emit_stream_end() {
        let mut parser = AnthropicSseParser::new();
        let mut chunks = parser
            .feed(
            "event: message_start\ndata: {\"message\":{\"id\":\"msg_truncated\",\"usage\":{\"input_tokens\":1}}}",
        )
            .unwrap();
        chunks.extend(parser.feed(
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}",
        ).unwrap());
        chunks.extend(parser.feed("").unwrap());

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::TextDelta { text } if text == "partial"
        )));
        assert!(!chunks
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::StreamEnd)));
    }

    #[test]
    fn text_content_block_stop_remains_generic_content() {
        let mut parser = AnthropicSseParser::new();
        parser
            .feed(
                "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}",
            )
            .unwrap();

        let chunks = parser
            .feed("event: content_block_stop\ndata: {\"index\":0}")
            .unwrap();

        assert!(matches!(
            chunks.as_slice(),
            [ProviderChunk::ContentBlockStop { index: 0 }]
        ));
    }

    #[test]
    fn tool_content_block_stop_is_typed_as_tool_completion() {
        let mut parser = AnthropicSseParser::new();
        parser
            .feed(
                "event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read_file\",\"input\":{}}}",
            )
            .unwrap();

        let chunks = parser
            .feed("event: content_block_stop\ndata: {\"index\":1}")
            .unwrap();

        assert!(matches!(
            chunks.last(),
            Some(ProviderChunk::ToolCallStop { index: 1 })
        ));
    }

    #[test]
    fn missing_or_malformed_tool_index_is_invalid() {
        for index in ["null", "\"zero\""] {
            let mut parser = AnthropicSseParser::new();
            parser.feed(&format!(
                "event: content_block_start\ndata: {{\"index\":{index},\"content_block\":{{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read_file\",\"input\":{{}}}}}}"
            )).unwrap();
            let chunks = parser
                .feed("event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}")
                .unwrap();
            assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
        }

        let mut parser = AnthropicSseParser::new();
        let chunks = parser.feed(
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read_file\",\"input\":{}}}",
        ).unwrap();
        assert!(chunks
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::ToolCallStart { index: 0, .. })));
    }
}
