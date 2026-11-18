use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::config::{ProviderFormat, ProviderProfile};
use crate::context::{CanonicalMessage, MessageBlock, Role};
use crate::event::ModelStopReason;

use super::chunk::ProviderChunk;
use super::replay::{native_replay, validate_replay_size, ReplayBudget, MAX_REPLAY_ITEMS};
use super::report::ReportAccumulator;
use super::sse::{parse_sse_frame, stream_sse_events};
use super::trace::ProviderTraceDirection;
use super::transport::{send_prepared, TransportResponse};
use super::types::{PreparedProviderRequest, ProviderFailure, ProviderRequest, ResolvedProvider};

const TRUNCATED_STREAM_MESSAGE: &str = "provider stream ended before response.completed";

pub(crate) fn responses_url(base_url: &str) -> String {
    format!("{}/responses", base_url.trim_end_matches('/'))
}

fn convert_to_input_items(message: &CanonicalMessage) -> Vec<Value> {
    if message.role == Role::Assistant {
        if let Some(items) = native_replay(message, ProviderFormat::OpenAiResponses)
            .and_then(|value| value.get("output"))
            .and_then(Value::as_array)
        {
            return items.clone();
        }
    }
    match message.role {
        Role::User => {
            let mut items = Vec::new();
            let mut text_parts = Vec::new();

            for block in &message.blocks {
                match block {
                    MessageBlock::Text(text) => text_parts.push(text.clone()),
                    MessageBlock::ToolResult(result) => {
                        if !text_parts.is_empty() {
                            items.push(json!({
                                "type": "message",
                                "role": "user",
                                "content": input_text_parts(&text_parts),
                            }));
                            text_parts.clear();
                        }
                        items.push(json!({
                            "type": "function_call_output",
                            "call_id": result.tool_call_id,
                            "output": result.model_content,
                        }));
                    }
                    MessageBlock::ToolUse(_)
                    | MessageBlock::Thinking(_)
                    | MessageBlock::ProviderReplay { .. } => {}
                }
            }

            if !text_parts.is_empty() {
                items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": input_text_parts(&text_parts),
                }));
            }

            items
        }
        Role::Assistant => {
            let mut items = Vec::new();
            let text = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Text(text) => Some(text.as_str()),
                    MessageBlock::ToolUse(_)
                    | MessageBlock::ToolResult(_)
                    | MessageBlock::Thinking(_) => None,
                    MessageBlock::ProviderReplay { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("");

            if !text.is_empty() {
                items.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": text,
                }));
            }

            items.extend(message.blocks.iter().filter_map(|block| match block {
                MessageBlock::ToolUse(tool_use) => Some(json!({
                    "type": "function_call",
                    "call_id": tool_use.id,
                    "name": tool_use.name,
                    "arguments": serde_json::to_string(&tool_use.args).expect("tool arguments are JSON"),
                })),
                MessageBlock::Text(_) | MessageBlock::ToolResult(_) | MessageBlock::Thinking(_) | MessageBlock::ProviderReplay { .. } => {
                    None
                }
            }));

            items
        }
    }
}

fn input_text_parts(text_parts: &[String]) -> Value {
    json!(text_parts
        .iter()
        .map(|text| json!({"type": "input_text", "text": text}))
        .collect::<Vec<_>>())
}

pub(crate) fn render_body(request: &ProviderRequest<'_>) -> Value {
    let mut input_items = Vec::new();

    for message in &request.assembly.prelude_messages {
        input_items.extend(convert_to_input_items(message));
    }
    if let Some(summary) = &request.assembly.handoff_summary {
        let template = &request.catalog.runtime["handoff-context"].text;
        let content = template.replace("{{handoff_summary}}", summary);
        input_items.push(json!({
            "role": "user",
            "content": content,
        }));
    }
    for message in &request.assembly.history {
        input_items.extend(convert_to_input_items(message));
    }

    let mut body = json!({
        "model": request.model,
        "instructions": request.assembly.system_prompt,
        "input": input_items,
    });

    if let Some(max_tokens) = request.max_output_tokens {
        body["max_output_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if request.think_level != crate::config::ThinkLevel::Off {
        let effort = match request.think_level.as_str() {
            "low" => "low",
            "medium" => "medium",
            "high" => "xhigh",
            _ => "medium",
        };
        body["reasoning"] = json!({
            "effort": effort,
        });
    }
    if !request.assembly.tools.is_empty() {
        body["tools"] = json!(request
            .assembly
            .tools
            .iter()
            .map(|schema| {
                json!({
                    "type": "function",
                    "name": schema.name,
                    "description": schema.description,
                    "parameters": schema.input_schema,
                })
            })
            .collect::<Vec<_>>());
    }

    body
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
    let parser = Arc::new(Mutex::new(OpenAiResponsesSseParser::new(
        config.capabilities.profile == ProviderProfile::DeepSeek,
    )));
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

struct ResponseFunctionCall {
    item_id: Option<String>,
    call_id: String,
    name: String,
    arguments: String,
    done_arguments: Option<String>,
    item_done: bool,
}

enum ResponseTerminalEvent {
    Completed,
    Incomplete,
    Failed,
    Error,
}

struct OpenAiResponsesSseParser {
    public_reasoning_text: bool,
    chunks: Vec<ProviderChunk>,
    started: bool,
    terminal_event: Option<ResponseTerminalEvent>,
    output_item_types: BTreeMap<u64, String>,
    item_id_indices: BTreeMap<String, u64>,
    function_calls: BTreeMap<u64, ResponseFunctionCall>,
    invalid_function_calls: bool,
    replay_items: BTreeMap<u64, Value>,
    replay_budget: ReplayBudget,
    summary_lengths: BTreeMap<(u64, u64), usize>,
    reports: ReportAccumulator,
}

impl OpenAiResponsesSseParser {
    fn new(public_reasoning_text: bool) -> Self {
        Self {
            public_reasoning_text,
            chunks: Vec::new(),
            started: false,
            terminal_event: None,
            output_item_types: BTreeMap::new(),
            item_id_indices: BTreeMap::new(),
            function_calls: BTreeMap::new(),
            invalid_function_calls: false,
            replay_items: BTreeMap::new(),
            replay_budget: ReplayBudget::default(),
            summary_lengths: BTreeMap::new(),
            reports: ReportAccumulator::default(),
        }
    }

    fn publish_item_summary(&mut self, index: u64, item: &Value) {
        if item["type"] == "reasoning" {
            if let Some(parts) = item["summary"].as_array() {
                for (part, value) in parts.iter().enumerate() {
                    if value["type"] == "summary_text" {
                        if let Some(text) = value["text"].as_str() {
                            self.publish_summary(index, part as u64, text, true);
                        }
                    }
                }
            }
        }
    }

    fn publish_summary(&mut self, item: u64, part: u64, text: &str, complete: bool) {
        if text.is_empty() {
            return;
        }
        let key = (item, part);
        let is_new = !self.summary_lengths.contains_key(&key);
        if is_new && self.summary_lengths.len() >= MAX_REPLAY_ITEMS {
            return;
        }
        let separate = is_new && !self.summary_lengths.is_empty();
        let length = self.summary_lengths.entry(key).or_default();
        let delta = if complete {
            text.get(*length..).unwrap_or_default()
        } else {
            text
        };
        if !delta.is_empty() {
            *length += delta.len();
            let text = if separate {
                format!("\n\n{delta}")
            } else {
                delta.into()
            };
            self.chunks.push(ProviderChunk::ThinkingDelta { text });
        }
    }

    fn invalidate_tool_stream(&mut self) {
        self.invalid_function_calls = true;
        self.chunks.push(ProviderChunk::InvalidToolStream);
    }

    fn item_identity(value: Option<&Value>) -> Result<Option<String>, ()> {
        let Some(value) = value else {
            return Ok(None);
        };
        let id = match value.get("id") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if !id.trim().is_empty() => Some(id.clone()),
            Some(_) => return Err(()),
        };
        let item_id = match value.get("item_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(item_id)) if !item_id.trim().is_empty() => Some(item_id.clone()),
            Some(_) => return Err(()),
        };
        match (id, item_id) {
            (Some(id), Some(item_id)) if id != item_id => Err(()),
            (Some(id), _) | (_, Some(id)) => Ok(Some(id)),
            (None, None) => Ok(None),
        }
    }

    fn event_item_identity(data: &Value, item: Option<&Value>) -> Result<Option<String>, ()> {
        let event_id = match data.get("item_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(item_id)) if !item_id.trim().is_empty() => Some(item_id.clone()),
            Some(_) => return Err(()),
        };
        let item_id = Self::item_identity(item)?;
        match (event_id, item_id) {
            (Some(event_id), Some(item_id)) if event_id != item_id => Err(()),
            (Some(event_id), _) | (_, Some(event_id)) => Ok(Some(event_id)),
            (None, None) => Ok(None),
        }
    }

    fn validate_function_identity(
        &mut self,
        index: u64,
        data: &Value,
        item: Option<&Value>,
        require_if_known: bool,
    ) -> bool {
        let identity = match Self::event_item_identity(data, item) {
            Ok(identity) => identity,
            Err(()) => {
                self.invalidate_tool_stream();
                return false;
            }
        };
        let Some(call) = self.function_calls.get(&index) else {
            self.invalidate_tool_stream();
            return false;
        };
        let expected = call.item_id.clone();
        if require_if_known && expected.is_some() && identity.is_none() {
            self.invalidate_tool_stream();
            return false;
        }
        if expected
            .as_deref()
            .zip(identity.as_deref())
            .is_some_and(|(expected, actual)| expected != actual)
        {
            self.invalidate_tool_stream();
            return false;
        }
        if let Some(identity) = identity {
            if self
                .item_id_indices
                .get(&identity)
                .is_some_and(|owner| *owner != index)
            {
                self.invalidate_tool_stream();
                return false;
            }
            if expected.is_none() {
                self.function_calls
                    .get_mut(&index)
                    .expect("validated function call")
                    .item_id = Some(identity.clone());
            }
            self.item_id_indices.insert(identity, index);
        }
        true
    }

    fn terminal_identity_matches(
        &self,
        index: u64,
        call: &ResponseFunctionCall,
        identity: Option<&str>,
    ) -> bool {
        if let Some(identity) = identity {
            if self
                .item_id_indices
                .get(identity)
                .is_some_and(|owner| *owner != index)
            {
                return false;
            }
        }
        match (call.item_id.as_deref(), identity) {
            (Some(expected), Some(actual)) => expected == actual,
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    fn feed(&mut self, frame: &str) -> Result<Vec<ProviderChunk>, ProviderFailure> {
        if frame.is_empty() {
            return Ok(self.take_chunks());
        }

        let Some(frame) = parse_sse_frame(frame) else {
            return Ok(self.take_chunks());
        };
        if frame.data.is_empty() {
            return Ok(self.take_chunks());
        }
        if frame.data.trim() == "[DONE]" {
            return if matches!(
                self.terminal_event,
                Some(
                    ResponseTerminalEvent::Completed
                        | ResponseTerminalEvent::Incomplete
                        | ResponseTerminalEvent::Failed
                )
            ) {
                Ok(self.take_chunks())
            } else {
                Err(super::prepare::invalid_request(
                    "provider emitted [DONE] before a terminal response event",
                ))
            };
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
            "response.created" if !self.started => {
                let rid = data
                    .get("response")
                    .and_then(|r| r.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.chunks
                    .push(ProviderChunk::StreamStart { request_id: rid });
                self.started = true;
                if let Some(response) = data.get("response") {
                    self.push_usage(response)?;
                }
            }
            "response.output_item.added" => {
                let Some(item) = data.get("item") else {
                    return Ok(self.take_chunks());
                };
                self.replay_budget.add(item)?;
                if self.output_item_types.len() >= MAX_REPLAY_ITEMS {
                    return Err(super::prepare::invalid_request(
                        "too many native response items",
                    ));
                }
                let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                    return Ok(self.take_chunks());
                };
                let Some(index) = data.get("output_index").and_then(Value::as_u64) else {
                    if item_type == "function_call" {
                        self.invalidate_tool_stream();
                    }
                    return Ok(self.take_chunks());
                };
                if self
                    .output_item_types
                    .insert(index, item_type.to_string())
                    .is_some()
                {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                }
                let item_identity = match Self::event_item_identity(&data, Some(item)) {
                    Ok(identity) => identity,
                    Err(()) => {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    }
                };
                if let Some(item_identity) = item_identity.as_deref() {
                    if self
                        .item_id_indices
                        .get(item_identity)
                        .is_some_and(|owner| *owner != index)
                    {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    }
                    self.item_id_indices
                        .insert(item_identity.to_string(), index);
                }
                if item_type == "function_call" {
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.function_calls.insert(
                        index,
                        ResponseFunctionCall {
                            item_id: item_identity,
                            call_id: call_id.clone(),
                            name: name.clone(),
                            arguments: String::new(),
                            done_arguments: None,
                            item_done: false,
                        },
                    );
                    self.chunks.push(ProviderChunk::ToolCallStart {
                        index,
                        id: call_id,
                        name,
                    });
                } else if item_type == "reasoning" {
                    self.chunks.push(ProviderChunk::Reasoning);
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    if !delta.is_empty() {
                        self.chunks.push(ProviderChunk::TextDelta {
                            text: delta.to_string(),
                        });
                    }
                }
            }
            "response.reasoning_text.delta" => {
                if let Some(text) = data["delta"].as_str().filter(|text| !text.is_empty()) {
                    self.chunks.push(if self.public_reasoning_text {
                        ProviderChunk::ThinkingDelta { text: text.into() }
                    } else {
                        ProviderChunk::Reasoning
                    });
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_summary_text.done" => {
                let complete = event_type.ends_with(".done");
                if let (Some(item), Some(part), Some(text)) = (
                    data["output_index"].as_u64(),
                    data["summary_index"].as_u64(),
                    data[if complete { "text" } else { "delta" }].as_str(),
                ) {
                    self.publish_summary(item, part, text, complete);
                }
            }
            "response.function_call_arguments.delta" => {
                let Some(index) = data.get("output_index").and_then(Value::as_u64) else {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                };
                let Some(delta) = data.get("delta").and_then(Value::as_str) else {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                };
                self.replay_budget.add(&Value::String(delta.into()))?;
                if !self.validate_function_identity(index, &data, None, true) {
                    return Ok(self.take_chunks());
                }
                let valid = self
                    .function_calls
                    .get(&index)
                    .is_some_and(|call| !call.item_done && call.done_arguments.is_none());
                if !valid {
                    self.invalidate_tool_stream();
                } else if !delta.is_empty() {
                    self.function_calls
                        .get_mut(&index)
                        .expect("validated function call")
                        .arguments
                        .push_str(delta);
                    self.chunks.push(ProviderChunk::ToolCallArgDelta {
                        index,
                        fragment: delta.to_string(),
                    });
                }
            }
            "response.function_call_arguments.done" => {
                let Some(index) = data.get("output_index").and_then(Value::as_u64) else {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                };
                let item = data.get("item");
                if !self.validate_function_identity(index, &data, item, true) {
                    return Ok(self.take_chunks());
                }
                let top_level_arguments = match data.get("arguments") {
                    None => None,
                    Some(Value::String(arguments)) => Some(arguments),
                    Some(_) => {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    }
                };
                let arguments = if let Some(item) = item {
                    let matches_call = item.get("type").and_then(Value::as_str)
                        == Some("function_call")
                        && item.get("call_id").and_then(Value::as_str)
                            == self
                                .function_calls
                                .get(&index)
                                .map(|call| call.call_id.as_str())
                        && item.get("name").and_then(Value::as_str)
                            == self
                                .function_calls
                                .get(&index)
                                .map(|call| call.name.as_str());
                    let Some(item_arguments) = item.get("arguments").and_then(Value::as_str) else {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    };
                    if !matches_call
                        || top_level_arguments.is_some_and(|arguments| arguments != item_arguments)
                    {
                        self.invalidate_tool_stream();
                        return Ok(self.take_chunks());
                    }
                    item_arguments
                } else if let Some(arguments) = top_level_arguments {
                    arguments
                } else {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                };
                self.replay_budget.add(&Value::String(arguments.into()))?;
                let Some(call) = self.function_calls.get_mut(&index) else {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                };
                if call.item_done || call.done_arguments.is_some() {
                    self.invalidate_tool_stream();
                    return Ok(self.take_chunks());
                }
                if call.arguments.is_empty() {
                    call.arguments.push_str(arguments);
                    if !arguments.is_empty() {
                        self.chunks.push(ProviderChunk::ToolCallArgDelta {
                            index,
                            fragment: arguments.to_string(),
                        });
                    }
                } else if call.arguments != arguments {
                    self.invalid_function_calls = true;
                }
                call.done_arguments = Some(arguments.to_string());
                if self.invalid_function_calls {
                    self.chunks.push(ProviderChunk::InvalidToolStream);
                }
            }
            "response.output_item.done" => {
                if let (Some(index), Some(item)) = (
                    data.get("output_index").and_then(Value::as_u64),
                    data.get("item"),
                ) {
                    self.replay_budget.add(item)?;
                    self.publish_item_summary(index, item);
                    self.replay_items.insert(index, item.clone());
                }
                self.finish_output_item(&data);
            }
            "response.completed" => {
                if let Some(resp) = data.get("response") {
                    self.push_usage(resp)?;
                    let reason = self.completed_reason(resp);
                    if reason != ModelStopReason::InvalidResponse {
                        if let Some(items) = resp["output"].as_array() {
                            for (index, item) in items.iter().enumerate() {
                                self.publish_item_summary(index as u64, item);
                            }
                        }
                        let output = resp
                            .get("output")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_else(|| self.replay_items.values().cloned().collect());
                        if !output.is_empty() {
                            let value = json!({"output": output});
                            validate_replay_size(&value)?;
                            self.chunks.push(ProviderChunk::Replay {
                                protocol: ProviderFormat::OpenAiResponses,
                                value,
                            });
                        }
                    }
                    self.chunks.push(ProviderChunk::StopReason { reason });
                } else {
                    self.chunks.push(ProviderChunk::StopReason {
                        reason: ModelStopReason::InvalidResponse,
                    });
                }
                self.terminal_event = Some(ResponseTerminalEvent::Completed);
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "response.incomplete" => {
                if let Some(resp) = data.get("response") {
                    self.push_usage(resp)?;
                    let reason = resp
                        .get("incomplete_details")
                        .and_then(|details| details.get("reason"))
                        .and_then(Value::as_str)
                        .and_then(ModelStopReason::from_wire)
                        .unwrap_or(ModelStopReason::Incomplete);
                    self.chunks.push(ProviderChunk::StopReason { reason });
                } else {
                    self.chunks.push(ProviderChunk::StopReason {
                        reason: ModelStopReason::Incomplete,
                    });
                }
                self.terminal_event = Some(ResponseTerminalEvent::Incomplete);
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "response.failed" | "error" => {
                if let Some(response) = data.get("response") {
                    self.push_usage(response)?;
                }
                let error = if event_type == "error" {
                    Some(&data)
                } else {
                    data.get("response")
                        .and_then(|response| response.get("error"))
                };
                let code = error
                    .and_then(|value| value.get("code").or_else(|| value.get("type")))
                    .and_then(Value::as_str)
                    .unwrap_or(event_type)
                    .to_string();
                let message = error
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("server error")
                    .to_string();
                self.chunks
                    .push(ProviderChunk::ServerError { code, message });
                self.terminal_event = Some(if event_type == "response.failed" {
                    ResponseTerminalEvent::Failed
                } else {
                    ResponseTerminalEvent::Error
                });
            }
            _ => {}
        }

        Ok(self.take_chunks())
    }

    fn push_usage(&mut self, response: &Value) -> Result<(), ProviderFailure> {
        if let Some(report) = self.reports.observe(response)? {
            self.chunks.push(ProviderChunk::Report { report });
        }
        if response.get("usage").is_some_and(Value::is_object) {
            if let Some(usage) = self.reports.usage(ProviderFormat::OpenAiResponses, true) {
                self.chunks.push(usage);
            }
        }
        Ok(())
    }

    fn finish_output_item(&mut self, data: &Value) {
        let Some(item) = data.get("item") else {
            if data
                .get("output_index")
                .and_then(Value::as_u64)
                .is_some_and(|index| self.function_calls.contains_key(&index))
            {
                self.invalidate_tool_stream();
            }
            return;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            if data
                .get("output_index")
                .and_then(Value::as_u64)
                .is_some_and(|index| self.function_calls.contains_key(&index))
            {
                self.invalidate_tool_stream();
            }
            return;
        };
        let Some(index) = data.get("output_index").and_then(Value::as_u64) else {
            if item_type == "function_call" {
                self.invalidate_tool_stream();
            }
            return;
        };
        if self.output_item_types.get(&index).map(String::as_str) != Some(item_type) {
            self.invalidate_tool_stream();
            return;
        }
        if item_type != "function_call" {
            self.chunks.push(ProviderChunk::ContentBlockStop { index });
            return;
        }

        if !self.validate_function_identity(index, data, Some(item), true) {
            return;
        }

        let Some(arguments) = item.get("arguments").and_then(Value::as_str) else {
            self.invalidate_tool_stream();
            return;
        };
        let Some(call) = self.function_calls.get_mut(&index) else {
            self.invalidate_tool_stream();
            return;
        };
        let matches_call = item.get("call_id").and_then(Value::as_str) == Some(&call.call_id)
            && item.get("name").and_then(Value::as_str) == Some(&call.name)
            && !call.item_done
            && call
                .done_arguments
                .as_deref()
                .is_none_or(|done| done == arguments);
        if !matches_call {
            self.invalid_function_calls = true;
        }
        if call.arguments.is_empty() {
            call.arguments.push_str(arguments);
            if !arguments.is_empty() {
                self.chunks.push(ProviderChunk::ToolCallArgDelta {
                    index,
                    fragment: arguments.to_string(),
                });
            }
        } else if call.arguments != arguments {
            self.invalid_function_calls = true;
        }
        call.item_done = true;
        if self.invalid_function_calls {
            self.chunks.push(ProviderChunk::InvalidToolStream);
        }
        self.chunks.push(ProviderChunk::ToolCallStop { index });
    }

    fn completed_reason(&self, response: &Value) -> ModelStopReason {
        if response.get("status").and_then(Value::as_str) != Some("completed") {
            return response
                .get("status")
                .and_then(Value::as_str)
                .and_then(ModelStopReason::from_wire)
                .unwrap_or(ModelStopReason::InvalidResponse);
        }
        if self.invalid_function_calls {
            return ModelStopReason::InvalidResponse;
        }

        let output = response.get("output").and_then(Value::as_array);
        let Some(output) = output else {
            return if self.function_calls.is_empty() {
                ModelStopReason::EndTurn
            } else {
                ModelStopReason::InvalidResponse
            };
        };
        let mut expected_calls = BTreeMap::new();
        let mut terminal_item_ids: BTreeMap<String, u64> = BTreeMap::new();
        for (index, item) in output.iter().enumerate() {
            let index = index as u64;
            let item_identity = match Self::event_item_identity(&Value::Null, Some(item)) {
                Ok(identity) => identity,
                Err(()) => return ModelStopReason::InvalidResponse,
            };
            if let Some(item_identity) = item_identity.as_deref() {
                if terminal_item_ids
                    .insert(item_identity.to_string(), index)
                    .is_some_and(|owner| owner != index)
                    || self
                        .item_id_indices
                        .get(item_identity)
                        .is_some_and(|owner| *owner != index)
                {
                    return ModelStopReason::InvalidResponse;
                }
            }
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                continue;
            }
            let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                return ModelStopReason::InvalidResponse;
            };
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                return ModelStopReason::InvalidResponse;
            };
            let Some(arguments) = item.get("arguments").and_then(Value::as_str) else {
                return ModelStopReason::InvalidResponse;
            };
            if expected_calls
                .insert(index, (call_id, name, arguments, item_identity))
                .is_some()
            {
                return ModelStopReason::InvalidResponse;
            }
        }
        if expected_calls.len() != self.function_calls.len() {
            return ModelStopReason::InvalidResponse;
        }
        for (index, (call_id, name, arguments, item_identity)) in expected_calls {
            let Some(call) = self.function_calls.get(&index) else {
                return ModelStopReason::InvalidResponse;
            };
            if self.output_item_types.get(&index).map(String::as_str) != Some("function_call")
                || call.call_id != call_id
                || call.name != name
                || call.arguments != arguments
                || !call.item_done
                || !self.terminal_identity_matches(index, call, item_identity.as_deref())
                || call
                    .done_arguments
                    .as_deref()
                    .is_some_and(|done| done != arguments)
            {
                return ModelStopReason::InvalidResponse;
            }
        }
        if self.function_calls.is_empty() {
            ModelStopReason::EndTurn
        } else {
            ModelStopReason::ToolUse
        }
    }

    fn finish(&mut self) -> Result<Vec<ProviderChunk>, ProviderFailure> {
        if self.terminal_event.is_some() {
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
#[allow(dead_code)] // included via include!() in integration tests
pub(crate) fn parse_responses_sse(body: &str) -> Vec<ProviderChunk> {
    let mut parser = OpenAiResponsesSseParser::new(false);
    let mut chunks = Vec::new();
    for frame in body.split("\n\n") {
        chunks.extend(parser.feed(frame.trim()).unwrap());
    }
    chunks.extend(parser.feed("").unwrap());
    chunks
}

#[cfg(test)]
#[path = "openai_responses/tests.rs"]
mod tests;
