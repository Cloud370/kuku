use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::config::{ProviderFormat, ProviderProfile};
use crate::context::{CanonicalMessage, MessageBlock, Role};
use crate::event::ModelStopReason;

use super::chunk::ProviderChunk;
use super::replay::{MAX_REPLAY_ITEMS, ReplayBudget, native_replay, validate_replay_size};
use super::report::ReportAccumulator;
use super::sse::{parse_sse_frame, stream_sse_events};
use super::transport::{TransportResponse, send_prepared};
use super::types::{PreparedProviderRequest, ProviderFailure, ProviderRequest, ResolvedProvider};

const TRUNCATED_STREAM_MESSAGE: &str = "provider stream ended before message_stop";

#[cfg(test)]
#[path = "anthropic_thinking_tests.rs"]
mod thinking_tests;

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
        "stream": false,
        "system": [{
            "type": "text",
            "text": request.assembly.system_prompt,
        }],
    });

    if let Some(max_tokens) = request.max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
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
        body["tools"] = json!(
            request
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
                .collect::<Vec<_>>()
        );
    }
    body
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

pub(crate) async fn stream_prepared(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    trace_metadata: Option<super::trace::ProviderTraceMetadata>,
) -> Result<super::ProviderChunkStream, ProviderFailure> {
    let TransportResponse { response, trace } =
        send_prepared(config, prepared, trace_metadata).await?;
    let parser = Arc::new(Mutex::new(AnthropicSseParser::new(
        config.capabilities.profile == ProviderProfile::Anthropic,
        config.capabilities.profile == ProviderProfile::DeepSeek
            || (prepared.body.as_json()["thinking"]["display"] == "summarized"
                && config.capabilities.profile == ProviderProfile::Anthropic),
    )));
    let frame_parser = Arc::clone(&parser);
    let eof_parser = Arc::clone(&parser);
    Ok(stream_sse_events(
        response,
        trace,
        move |frame| frame_parser.lock().unwrap().feed(frame),
        move || eof_parser.lock().unwrap().finish(),
    ))
}

struct AnthropicSseParser {
    chunks: Vec<ProviderChunk>,
    thinking_signature_required_for_replay: bool,
    public_reasoning_text: bool,
    last_public_block: Option<u64>,
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
    fn new(thinking_signature_required_for_replay: bool, public_reasoning_text: bool) -> Self {
        Self {
            chunks: Vec::new(),
            thinking_signature_required_for_replay,
            public_reasoning_text,
            last_public_block: None,
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

    fn observe_thinking(&mut self, index: u64, text: &str) {
        if !self.public_reasoning_text || text.is_empty() {
            self.chunks.push(ProviderChunk::Reasoning);
            return;
        }
        let new_block = self
            .last_public_block
            .replace(index)
            .is_some_and(|last| last != index);
        let text = if new_block {
            format!("\n\n{text}")
        } else {
            text.to_owned()
        };
        self.chunks.push(ProviderChunk::ThinkingDelta { text });
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
            super::prepare::invalid_response("provider emitted malformed JSON event")
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
                    let Some(index) = data.get("index").and_then(Value::as_u64) else {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    };
                    self.replay_budget.add(block)?;
                    if self.content_blocks.len() >= MAX_REPLAY_ITEMS {
                        return Err(super::prepare::invalid_request(
                            "too many native content blocks",
                        ));
                    }
                    if self.content_blocks.insert(index, block.clone()).is_some() {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
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
                        Some("thinking") => self.observe_thinking(
                            index,
                            block["thinking"].as_str().unwrap_or_default(),
                        ),
                        Some("redacted_thinking") => self.chunks.push(ProviderChunk::Reasoning),
                        Some("tool_use") => {
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
                    let Some(index) = data.get("index").and_then(Value::as_u64) else {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    };
                    if !self.content_blocks.contains_key(&index)
                        || self.completed_blocks.contains(&index)
                    {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    }
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                            if self.append_native(index, "text", "text", text)? && !text.is_empty()
                            {
                                self.chunks.push(ProviderChunk::TextDelta {
                                    text: text.to_string(),
                                });
                            }
                        }
                        Some("thinking_delta") => {
                            let text = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                            if self.append_native(index, "thinking", "thinking", text)?
                                && !text.is_empty()
                            {
                                self.observe_thinking(index, text);
                            }
                        }
                        Some("signature_delta") => {
                            if let Some(signature) = delta.get("signature").and_then(Value::as_str)
                            {
                                self.append_native(index, "thinking", "signature", signature)?;
                            }
                        }
                        Some("input_json_delta") => {
                            let fragment = delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if !self.tool_blocks.contains(&index)
                                || self
                                    .content_blocks
                                    .get(&index)
                                    .and_then(|block| block.get("input"))
                                    .and_then(Value::as_object)
                                    .is_some_and(|input| !input.is_empty())
                            {
                                self.invalidate_tool_stream();
                            } else if !fragment.is_empty() {
                                self.replay_budget
                                    .add(&Value::String(fragment.to_string()))?;
                                self.tool_arguments
                                    .entry(index)
                                    .or_default()
                                    .push_str(fragment);
                                self.chunks.push(ProviderChunk::ToolCallArgDelta {
                                    index,
                                    fragment: fragment.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let Some(index) = data.get("index").and_then(Value::as_u64) else {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                };
                if !self.content_blocks.contains_key(&index) || !self.completed_blocks.insert(index)
                {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
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
                                self.invalidate_tool_stream();
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
                            if !self.invalid_tool_stream && !self.complete_content() {
                                self.invalidate_tool_stream();
                            }
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
                if !self.invalid_tool_stream && !self.complete_content() {
                    self.invalidate_tool_stream();
                }
                if !self.invalid_tool_stream && !self.content_blocks.is_empty() {
                    let content = self
                        .content_blocks
                        .values()
                        .filter(|block| {
                            !self.thinking_signature_required_for_replay
                                || block["type"] != "thinking"
                                || block["signature"]
                                    .as_str()
                                    .is_some_and(|signature| !signature.is_empty())
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let value = json!({"role": "assistant", "content": content});
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
                let error = data.get("error");
                let code = super::error::select_provider_error_code(
                    error
                        .and_then(|value| value.get("code"))
                        .and_then(Value::as_str),
                    error
                        .and_then(|value| value.get("type"))
                        .and_then(Value::as_str),
                )
                .unwrap_or("unknown")
                .to_string();
                let message = error
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

    fn complete_content(&self) -> bool {
        self.content_blocks.len() == self.completed_blocks.len()
    }

    fn invalidate_tool_stream(&mut self) {
        self.invalid_tool_stream = true;
        self.chunks.push(ProviderChunk::InvalidToolStream);
    }

    fn append_native(
        &mut self,
        index: u64,
        kind: &str,
        field: &str,
        fragment: &str,
    ) -> Result<bool, ProviderFailure> {
        let Some(block) = self
            .content_blocks
            .get_mut(&index)
            .filter(|block| block["type"] == kind)
        else {
            self.invalidate_tool_stream();
            return Ok(false);
        };
        self.replay_budget.add(&Value::String(fragment.into()))?;
        if let Some(Value::String(existing)) = block.get_mut(field) {
            existing.push_str(fragment);
        } else {
            block[field] = Value::String(fragment.to_string());
        }
        Ok(true)
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
            kind: crate::provider::types::ProviderFailureKind::InvalidResponse,
            message: TRUNCATED_STREAM_MESSAGE.to_string(),
            status: None,
            provider_request_id: None,
            retry_after: None,
            retryable: false,
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
    fn messages_initial_output_is_not_normalized_as_final_usage() {
        let mut parser = AnthropicSseParser::new(true, false);
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
        let mut parser = AnthropicSseParser::new(true, false);
        let mut chunks = parser
            .feed(
            "event: message_start\ndata: {\"message\":{\"id\":\"msg_truncated\",\"usage\":{\"input_tokens\":1}}}",
        )
            .unwrap();
        chunks.extend(parser.feed(
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ).unwrap());
        chunks.extend(parser.feed(
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}",
        ).unwrap());
        chunks.extend(parser.feed("").unwrap());

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::TextDelta { text } if text == "partial"
        )));
        assert!(
            !chunks
                .iter()
                .any(|chunk| matches!(chunk, ProviderChunk::StreamEnd))
        );
        let failure = parser.finish().unwrap_err();
        assert_eq!(
            crate::provider::types::ProviderFailureKind::InvalidResponse,
            failure.kind
        );
        assert!(!failure.retryable);
    }

    #[test]
    fn error_event_prefers_a_permanent_code_over_a_transient_type() {
        let mut parser = AnthropicSseParser::new(true, false);
        let chunks = parser
            .feed(concat!(
                "event: error\n",
                r#"data: {"error":{"code":"insufficient_quota","type":"overloaded_error","message":"quota exhausted"}}"#
            ))
            .unwrap();
        assert!(matches!(
            chunks.as_slice(),
            [ProviderChunk::ServerError { code, .. }] if code == "insufficient_quota"
        ));
    }

    #[test]
    fn text_content_block_stop_remains_generic_content() {
        let mut parser = AnthropicSseParser::new(true, false);
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
        let mut parser = AnthropicSseParser::new(true, false);
        parser
            .feed(
                "event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read_file\",\"input\":{}}}",
            )
            .unwrap();
        assert!(parser.feed(r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}"#).unwrap().is_empty());

        let chunks = parser
            .feed("event: content_block_stop\ndata: {\"index\":1}")
            .unwrap();

        assert!(matches!(chunks.as_slice(), [
            ProviderChunk::ToolCallArgDelta { index: 1, fragment },
            ProviderChunk::ToolCallStop { index: 1 }
        ] if fragment == "{}"));
    }

    #[test]
    fn a_completed_tool_cannot_dispatch_with_incomplete_thinking() {
        let mut parser = AnthropicSseParser::new(true, false);
        let initial = parser.feed(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"fixture"}}"#).unwrap();
        assert!(matches!(initial.as_slice(), [ProviderChunk::Reasoning]));
        parser.feed(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"fixture-signature"}}"#).unwrap();
        parser.feed(r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call","name":"read_file","input":{}}}"#).unwrap();
        parser
            .feed(r#"data: {"type":"content_block_stop","index":1}"#)
            .unwrap();
        let terminal = parser
            .feed(r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#)
            .unwrap();
        assert!(matches!(
            terminal.as_slice(),
            [
                ProviderChunk::InvalidToolStream,
                ProviderChunk::StopReason {
                    reason: ModelStopReason::InvalidResponse
                }
            ]
        ));
        let ended = parser.feed(r#"data: {"type":"message_stop"}"#).unwrap();
        assert!(matches!(ended.as_slice(), [ProviderChunk::StreamEnd]));
    }

    #[test]
    fn unsigned_thinking_is_valid_but_omitted_from_native_replay() {
        let mut parser = AnthropicSseParser::new(true, true);
        let mut chunks = parser
            .feed(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#)
            .unwrap();
        chunks.extend(parser.feed(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"fixture reasoning"}}"#).unwrap());
        chunks.extend(
            parser
                .feed(r#"data: {"type":"content_block_stop","index":0}"#)
                .unwrap(),
        );
        chunks.extend(parser.feed(r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#).unwrap());
        chunks.extend(parser.feed(r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}"#).unwrap());
        chunks.extend(
            parser
                .feed(r#"data: {"type":"content_block_stop","index":1}"#)
                .unwrap(),
        );

        let terminal = parser
            .feed(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
            .unwrap();
        assert!(matches!(
            terminal.as_slice(),
            [ProviderChunk::StopReason {
                reason: ModelStopReason::EndTurn
            }]
        ));
        chunks.extend(terminal);
        let ended = parser.feed(r#"data: {"type":"message_stop"}"#).unwrap();
        let replay = ended.iter().find_map(|chunk| match chunk {
            ProviderChunk::Replay { value, .. } => Some(value),
            _ => None,
        });
        assert_eq!(
            Some(&json!([{"type":"text","text":"answer"}])),
            replay.and_then(|value| value.get("content"))
        );
        assert!(
            ended
                .iter()
                .any(|chunk| matches!(chunk, ProviderChunk::StreamEnd))
        );
        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::ThinkingDelta { text } if text == "fixture reasoning"
        )));
        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::TextDelta { text } if text == "answer"
        )));
        assert!(
            !chunks
                .iter()
                .any(|chunk| matches!(chunk, ProviderChunk::InvalidToolStream))
        );
    }

    #[test]
    fn signed_thinking_remains_in_native_replay() {
        let mut parser = AnthropicSseParser::new(true, false);
        parser
            .feed(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"fixture reasoning"}}"#)
            .unwrap();
        parser.feed(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"fixture-signature"}}"#).unwrap();
        parser
            .feed(r#"data: {"type":"content_block_stop","index":0}"#)
            .unwrap();
        let terminal = parser
            .feed(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
            .unwrap();
        assert!(matches!(
            terminal.as_slice(),
            [ProviderChunk::StopReason {
                reason: ModelStopReason::EndTurn
            }]
        ));

        let ended = parser.feed(r#"data: {"type":"message_stop"}"#).unwrap();
        let replay = ended.iter().find_map(|chunk| match chunk {
            ProviderChunk::Replay { value, .. } => Some(value),
            _ => None,
        });
        assert_eq!(
            Some(
                &json!([{"type":"thinking","thinking":"fixture reasoning","signature":"fixture-signature"}])
            ),
            replay.and_then(|value| value.get("content"))
        );
    }

    #[test]
    fn compatible_connection_preserves_unsigned_thinking_replay() {
        let mut parser = AnthropicSseParser::new(false, false);
        parser
            .feed(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"fixture reasoning"}}"#)
            .unwrap();
        parser
            .feed(r#"data: {"type":"content_block_stop","index":0}"#)
            .unwrap();
        let terminal = parser
            .feed(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
            .unwrap();
        assert!(matches!(
            terminal.as_slice(),
            [ProviderChunk::StopReason {
                reason: ModelStopReason::EndTurn
            }]
        ));

        let ended = parser.feed(r#"data: {"type":"message_stop"}"#).unwrap();
        let replay = ended.iter().find_map(|chunk| match chunk {
            ProviderChunk::Replay { value, .. } => Some(value),
            _ => None,
        });
        assert_eq!(
            Some(&json!([{"type":"thinking","thinking":"fixture reasoning"}])),
            replay.and_then(|value| value.get("content"))
        );
    }

    #[test]
    fn text_delta_addressed_to_a_thinking_block_is_never_visible_text() {
        let mut parser = AnthropicSseParser::new(true, false);
        parser.feed(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#).unwrap();
        let chunks = parser.feed(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"private fixture"}}"#).unwrap();
        assert!(matches!(
            chunks.as_slice(),
            [ProviderChunk::InvalidToolStream]
        ));
    }

    #[test]
    fn missing_or_malformed_tool_index_is_invalid() {
        for index in ["null", "\"zero\""] {
            let mut parser = AnthropicSseParser::new(true, false);
            parser.feed(&format!(
                "event: content_block_start\ndata: {{\"index\":{index},\"content_block\":{{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read_file\",\"input\":{{}}}}}}"
            )).unwrap();
            let chunks = parser
                .feed("event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}")
                .unwrap();
            assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
        }

        let mut parser = AnthropicSseParser::new(true, false);
        let chunks = parser.feed(
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read_file\",\"input\":{}}}",
        ).unwrap();
        assert!(
            chunks
                .iter()
                .any(|chunk| matches!(chunk, ProviderChunk::ToolCallStart { index: 0, .. }))
        );
    }
}
