use super::http_client;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use wreq::header::{HeaderMap, HeaderValue};

use crate::context::{CanonicalMessage, MessageBlock, Role};
use crate::event::ModelStopReason;

use super::chunk::ProviderChunk;
use super::error::{classify_http_error, transport_error};
use super::sse::stream_sse_events;
use super::trace::{ProviderTrace, ProviderTraceDirection};
use super::types::{ProviderFailure, ProviderRequest, ResolvedProvider};

const TRUNCATED_STREAM_MESSAGE: &str = "provider stream ended before response.completed";

pub(crate) fn responses_url(base_url: &str) -> String {
    format!("{}/responses", base_url.trim_end_matches('/'))
}

fn convert_to_input_items(message: &CanonicalMessage) -> Vec<Value> {
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
                    MessageBlock::ToolUse(_) | MessageBlock::Thinking(_) => {}
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
                    "arguments": tool_use.args,
                })),
                MessageBlock::Text(_) | MessageBlock::ToolResult(_) | MessageBlock::Thinking(_) => {
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
    let mut body = render_body(request);
    body["stream"] = json!(true);
    let url = responses_url(&config.base_url);
    let client = http_client::api_client();
    let trace = ProviderTrace::from_request(
        trace_metadata.as_ref(),
        config.kind.as_str(),
        config.model.clone(),
    );
    let headers = openai_headers(config);

    if let Some(trace) = &trace {
        trace.record(
            ProviderTraceDirection::Request,
            Some(&url),
            Some(&headers),
            json!({ "body": body }),
        );
    }

    let response = client
        .post(url.clone())
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if let Some(trace) = &trace {
                trace.record(
                    ProviderTraceDirection::Error,
                    Some(&url),
                    None,
                    json!({ "error": error.to_string() }),
                );
            }
            transport_error(&error)
        })?;

    let status = response.status();
    let response_headers = response.headers().clone();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    if let Some(trace) = &trace {
        trace.record(
            ProviderTraceDirection::Response,
            Some(&url),
            Some(&response_headers),
            json!({ "status": status.as_u16() }),
        );
    }
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        if let Some(trace) = &trace {
            trace.record(
                ProviderTraceDirection::Error,
                Some(&url),
                Some(&response_headers),
                json!({ "status": status.as_u16(), "body": body_text }),
            );
        }
        let mut failure = classify_http_error(status.as_u16(), &body_text);
        failure.provider_request_id = request_id;
        return Err(failure);
    }

    let parser = Arc::new(Mutex::new(OpenAiResponsesSseParser::new()));
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

fn openai_headers(config: &ResolvedProvider) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    let authorization = format!("Bearer {}", config.api_key.expose());
    headers.insert(
        "authorization",
        HeaderValue::from_str(&authorization).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers
}

struct ResponseFunctionCall {
    item_id: Option<String>,
    call_id: String,
    name: String,
    arguments: String,
    done_arguments: Option<String>,
    item_done: bool,
}

struct OpenAiResponsesSseParser {
    chunks: Vec<ProviderChunk>,
    started: bool,
    completed: bool,
    output_item_types: BTreeMap<u64, String>,
    item_id_indices: BTreeMap<String, u64>,
    function_calls: BTreeMap<u64, ResponseFunctionCall>,
    invalid_function_calls: bool,
}

impl OpenAiResponsesSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            started: false,
            completed: false,
            output_item_types: BTreeMap::new(),
            item_id_indices: BTreeMap::new(),
            function_calls: BTreeMap::new(),
            invalid_function_calls: false,
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

        let mut event_type = "";
        let mut data_str = "";

        for line in frame.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = rest.trim();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_str = rest.trim();
            }
        }

        if data_str.is_empty() {
            return Ok(self.take_chunks());
        }

        let data: Value = match serde_json::from_str(data_str) {
            Ok(v) => v,
            Err(_) => return Ok(self.take_chunks()),
        };

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
            }
            "response.output_item.added" => {
                let Some(item) = data.get("item") else {
                    return Ok(self.take_chunks());
                };
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
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    if !delta.is_empty() {
                        self.chunks.push(ProviderChunk::TextDelta {
                            text: delta.to_string(),
                        });
                    }
                }
            }
            "response.reasoning_text.delta" => {
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    if !delta.is_empty() {
                        self.chunks.push(ProviderChunk::ThinkingDelta {
                            text: delta.to_string(),
                        });
                    }
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
                self.finish_output_item(&data);
            }
            "response.completed" => {
                if let Some(resp) = data.get("response") {
                    self.push_usage(resp);
                    self.chunks.push(ProviderChunk::StopReason {
                        reason: self.completed_reason(resp),
                    });
                } else {
                    self.chunks.push(ProviderChunk::StopReason {
                        reason: ModelStopReason::InvalidResponse,
                    });
                }
                self.completed = true;
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "response.incomplete" => {
                if let Some(resp) = data.get("response") {
                    self.push_usage(resp);
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
                self.completed = true;
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "response.failed" | "error" => {
                if let Some(response) = data.get("response") {
                    self.push_usage(response);
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
                self.completed = true;
            }
            _ => {}
        }

        Ok(self.take_chunks())
    }

    fn push_usage(&mut self, response: &Value) {
        if let Some(usage) = response.get("usage") {
            let input_tokens_total = usage.get("input_tokens").and_then(Value::as_u64);
            let cache_read_input_tokens = usage
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64);
            self.chunks.push(ProviderChunk::StreamUsage {
                input_tokens: match (input_tokens_total, cache_read_input_tokens) {
                    (Some(total), Some(cached)) => Some(total.saturating_sub(cached)),
                    (Some(total), None) => Some(total),
                    (None, _) => None,
                },
                output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
                cache_read_input_tokens,
                cache_creation_input_tokens: None,
            });
        }
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
        if self.completed {
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
    let mut parser = OpenAiResponsesSseParser::new();
    let mut chunks = Vec::new();
    for frame in body.split("\n\n") {
        chunks.extend(parser.feed(frame.trim()).unwrap());
    }
    chunks.extend(parser.feed("").unwrap());
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_usage_reports_cached_input_tokens() {
        let mut parser = OpenAiResponsesSseParser::new();
        let frame = concat!(
            "event: response.completed\n",
            r#"data: {"type":"response.completed","response":{"id":"resp_usage","status":"completed","usage":{"input_tokens":120,"input_tokens_details":{"cached_tokens":70},"output_tokens":30,"output_tokens_details":{"reasoning_tokens":12}}}}"#,
        );

        let chunks = parser.feed(frame).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StreamUsage {
                input_tokens: Some(50),
                output_tokens: Some(30),
                cache_read_input_tokens: Some(70),
                cache_creation_input_tokens: None
            }
        )));
        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::EndTurn
        )));
    }

    #[test]
    fn reasoning_text_delta_emits_thinking() {
        let mut parser = OpenAiResponsesSseParser::new();
        let frame = concat!(
            "event: response.reasoning_text.delta\n",
            r#"data: {"type":"response.reasoning_text.delta","item_id":"rs_123","output_index":0,"content_index":0,"delta":"considering"}"#,
        );

        let chunks = parser.feed(frame).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::ThinkingDelta { text } if text == "considering"
        )));
    }

    #[test]
    fn incomplete_response_is_terminal_and_preserves_usage() {
        let mut parser = OpenAiResponsesSseParser::new();
        let frame = concat!(
            "event: response.incomplete\n",
            r#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","incomplete_details":{"reason":"max_tokens"},"usage":{"input_tokens":120,"input_tokens_details":{"cached_tokens":70},"output_tokens":30}}}"#,
        );

        let mut chunks = parser.feed(frame).unwrap();
        chunks.extend(parser.finish().unwrap());

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StreamUsage {
                input_tokens: Some(50),
                output_tokens: Some(30),
                cache_read_input_tokens: Some(70),
                cache_creation_input_tokens: None
            }
        )));
        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::Length
        )));
        assert!(chunks
            .last()
            .is_some_and(|chunk| matches!(chunk, ProviderChunk::StreamEnd)));
    }

    #[test]
    fn incomplete_response_without_reason_is_explicitly_incomplete() {
        let mut parser = OpenAiResponsesSseParser::new();
        let frame = concat!(
            "event: response.incomplete\n",
            r#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","usage":{"input_tokens":1,"output_tokens":2}}}"#,
        );

        let chunks = parser.feed(frame).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::Incomplete
        )));
    }

    #[test]
    fn completed_response_with_matching_function_call_is_tool_use() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();
        parser
            .feed(concat!(
                "event: response.output_item.done\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
            ))
            .unwrap();

        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
            ))
            .unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::ToolUse
        )));
    }

    #[test]
    fn function_call_done_emits_typed_tool_completion() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();

        let chunks = parser
            .feed(concat!(
                "event: response.output_item.done\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
            ))
            .unwrap();

        assert!(matches!(
            chunks.as_slice(),
            [
                ProviderChunk::ToolCallArgDelta { index: 0, fragment },
                ProviderChunk::ToolCallStop { index: 0 }
            ] if fragment == "{}"
        ));
    }

    #[test]
    fn completed_response_with_missing_call_marker_item_is_invalid() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();
        parser
            .feed(concat!(
                "event: response.output_item.done\n",
                r#"data: {"output_index":0}"#,
            ))
            .unwrap();

        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
            ))
            .unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
        )));
    }

    #[test]
    fn completed_response_with_mismatched_call_marker_is_invalid() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
            ))
            .unwrap();
        parser
            .feed(concat!(
                "event: response.output_item.done\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();

        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_2","name":"read_file","arguments":"{}"}]}}"#,
            ))
            .unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
        )));
    }

    #[test]
    fn authoritative_done_arguments_are_emitted_once_without_deltas() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser.feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"write_file"}}"#,
        )).unwrap();

        let done_chunks = parser
            .feed(concat!(
                "event: response.function_call_arguments.done\n",
                r#"data: {"output_index":0,"arguments":"{\"path\":\"a.txt\"}"}"#,
            ))
            .unwrap();
        let item_chunks = parser.feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}}"#,
        )).unwrap();
        let terminal_chunks = parser.feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}]}}"#,
        )).unwrap();

        assert!(
            matches!(done_chunks.as_slice(), [ProviderChunk::ToolCallArgDelta { index: 0, fragment }] if fragment == "{\"path\":\"a.txt\"}")
        );
        assert!(matches!(
            item_chunks.as_slice(),
            [ProviderChunk::ToolCallStop { index: 0 }]
        ));
        assert!(terminal_chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::ToolUse)));
    }

    #[test]
    fn done_item_arguments_are_authoritative_without_deltas() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":""}}"#,
            ))
            .unwrap();

        let done_chunks = parser
            .feed(concat!(
                "event: response.function_call_arguments.done\n",
                r#"data: {"output_index":0,"item_id":"fc_1","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}}"#,
            ))
            .unwrap();
        let item_chunks = parser
            .feed(concat!(
                "event: response.output_item.done\n",
                r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}}"#,
            ))
            .unwrap();
        let terminal_chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[{"id":"fc_1","type":"function_call","call_id":"call_1","name":"write_file","arguments":"{\"path\":\"a.txt\"}"}]}}"#,
            ))
            .unwrap();

        assert!(matches!(
            done_chunks.as_slice(),
            [ProviderChunk::ToolCallArgDelta { index: 0, fragment }]
                if fragment == "{\"path\":\"a.txt\"}"
        ));
        assert!(matches!(
            item_chunks.as_slice(),
            [ProviderChunk::ToolCallStop { index: 0 }]
        ));
        assert!(terminal_chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::ToolUse
        )));
    }

    #[test]
    fn done_item_and_top_level_arguments_must_match() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();
        parser
            .feed(concat!(
                "event: response.function_call_arguments.done\n",
                r#"data: {"output_index":0,"item_id":"fc_1","arguments":"{\"path\":\"top-level\"}","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"item\"}"}}"#,
            ))
            .unwrap();
        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[]}}"#,
            ))
            .unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
        )));
    }

    #[test]
    fn done_item_identity_mismatch_is_invalid() {
        for (field, value) in [
            ("item_id", "fc_other"),
            ("call_id", "call_other"),
            ("name", "other_tool"),
        ] {
            let mut parser = OpenAiResponsesSseParser::new();
            parser
                .feed(concat!(
                    "event: response.output_item.added\n",
                    r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
                ))
                .unwrap();
            let item = format!(
                "{{\"id\":\"{}\",\"type\":\"function_call\",\"call_id\":\"{}\",\"name\":\"{}\",\"arguments\":\"{{}}\"}}",
                if field == "item_id" { value } else { "fc_1" },
                if field == "call_id" { value } else { "call_1" },
                if field == "name" { value } else { "read_file" },
            );
            parser
                .feed(&format!(
                    "event: response.function_call_arguments.done\ndata: {{\"output_index\":0,\"item_id\":\"fc_1\",\"item\":{item}}}"
                ))
                .unwrap();
            let chunks = parser
                .feed(concat!(
                    "event: response.completed\n",
                    r#"data: {"response":{"status":"completed","output":[]}}"#,
                ))
                .unwrap();
            assert!(chunks.iter().any(|chunk| matches!(
                chunk,
                ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
            )));
        }
    }

    #[test]
    fn terminal_item_identity_mismatch_is_invalid() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();
        parser
            .feed(concat!(
                "event: response.output_item.done\n",
                r#"data: {"output_index":0,"item":{"id":"fc_other","type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
            ))
            .unwrap();
        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[{"id":"fc_other","type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
            ))
            .unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
        )));
    }

    #[test]
    fn function_item_identity_cannot_be_reused_across_indices() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            ))
            .unwrap();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":1,"item":{"id":"fc_1","type":"function_call","call_id":"call_2","name":"read_file"}}"#,
            ))
            .unwrap();
        let chunks = parser
            .feed(concat!(
                "event: response.completed\n",
                r#"data: {"response":{"status":"completed","output":[]}}"#,
            ))
            .unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse
        )));
    }

    #[test]
    fn mismatched_argument_representations_are_invalid() {
        let cases = [
            (
                Some("event: response.function_call_arguments.delta\ndata: {\"output_index\":0,\"delta\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}"),
                "event: response.function_call_arguments.done\ndata: {\"output_index\":0,\"arguments\":\"{\\\"path\\\":\\\"b.txt\\\"}\"}",
                r#"{\"path\":\"b.txt\"}"#,
                r#"{\"path\":\"b.txt\"}"#,
            ),
            (
                None,
                "event: response.function_call_arguments.done\ndata: {\"output_index\":0,\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}",
                r#"{\"path\":\"b.txt\"}"#,
                r#"{\"path\":\"b.txt\"}"#,
            ),
        ];

        for (delta, done, item_arguments, output_arguments) in cases {
            let mut parser = OpenAiResponsesSseParser::new();
            parser.feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"write_file"}}"#,
            )).unwrap();
            if let Some(delta) = delta {
                parser.feed(delta).unwrap();
            }
            parser.feed(done).unwrap();
            parser.feed(&format!(
                "event: response.output_item.done\ndata: {{\"output_index\":0,\"item\":{{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\",\"arguments\":{}}}}}",
                serde_json::to_string(item_arguments).unwrap()
            )).unwrap();
            let chunks = parser.feed(&format!(
                "event: response.completed\ndata: {{\"response\":{{\"status\":\"completed\",\"output\":[{{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"write_file\",\"arguments\":{}}}]}}}}",
                serde_json::to_string(output_arguments).unwrap()
            )).unwrap();

            assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
        }
    }

    #[test]
    fn missing_done_or_terminal_arguments_are_invalid() {
        let cases = [(None, Some("{}")), (Some("{}"), None)];

        for (done_arguments, terminal_arguments) in cases {
            let mut parser = OpenAiResponsesSseParser::new();
            parser.feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
            )).unwrap();
            let done = done_arguments.map_or_else(
                || "event: response.function_call_arguments.done\ndata: {\"output_index\":0}".to_string(),
                |arguments| format!("event: response.function_call_arguments.done\ndata: {{\"output_index\":0,\"arguments\":{}}}", serde_json::to_string(arguments).unwrap()),
            );
            parser.feed(&done).unwrap();
            parser.feed(concat!(
                "event: response.output_item.done\n",
                r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
            )).unwrap();
            let output_item = terminal_arguments.map_or_else(
                || r#"{"type":"function_call","call_id":"call_1","name":"read_file"}"#.to_string(),
                |arguments| format!(r#"{{"type":"function_call","call_id":"call_1","name":"read_file","arguments":{}}}"#, serde_json::to_string(arguments).unwrap()),
            );
            let chunks = parser.feed(&format!(
                "event: response.completed\ndata: {{\"response\":{{\"status\":\"completed\",\"output\":[{output_item}]}}}}"
            )).unwrap();

            assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
        }
    }

    #[test]
    fn output_index_reused_across_item_types_is_invalid() {
        let mut parser = OpenAiResponsesSseParser::new();
        parser
            .feed(concat!(
                "event: response.output_item.added\n",
                r#"data: {"output_index":0,"item":{"type":"message","id":"msg_1"}}"#,
            ))
            .unwrap();
        parser.feed(concat!(
            "event: response.output_item.added\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#,
        )).unwrap();
        parser
            .feed(concat!(
                "event: response.function_call_arguments.done\n",
                r#"data: {"output_index":0,"arguments":"{}"}"#,
            ))
            .unwrap();
        parser.feed(concat!(
            "event: response.output_item.done\n",
            r#"data: {"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}}"#,
        )).unwrap();
        let chunks = parser.feed(concat!(
            "event: response.completed\n",
            r#"data: {"response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{}"}]}}"#,
        )).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
    }

    #[test]
    fn missing_or_malformed_tool_output_index_is_invalid() {
        for output_index in ["null", "\"zero\""] {
            let mut parser = OpenAiResponsesSseParser::new();
            parser.feed(&format!(
                "event: response.output_item.added\ndata: {{\"output_index\":{output_index},\"item\":{{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read_file\"}}}}"
            )).unwrap();
            let chunks = parser
                .feed(concat!(
                    "event: response.completed\n",
                    r#"data: {"response":{"status":"completed","output":[]}}"#,
                ))
                .unwrap();
            assert!(chunks.iter().any(|chunk| matches!(chunk, ProviderChunk::StopReason { reason } if reason == &ModelStopReason::InvalidResponse)));
        }
    }

    #[test]
    fn failed_response_is_terminal_and_preserves_error_details() {
        let mut parser = OpenAiResponsesSseParser::new();
        let frame = concat!(
            "event: response.failed\n",
            r#"data: {"type":"response.failed","response":{"id":"resp_failed","status":"failed","error":{"code":"server_error","message":"generation failed"}}}"#,
        );

        let chunks = parser.feed(frame).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::ServerError { code, message }
                if code == "server_error" && message == "generation failed"
        )));
        assert!(parser.finish().is_ok());
    }

    #[test]
    fn error_event_preserves_top_level_error_details() {
        let mut parser = OpenAiResponsesSseParser::new();
        let frame = concat!(
            "event: error\n",
            r#"data: {"type":"error","code":"server_error","message":"stream failed","param":null}"#,
        );

        let chunks = parser.feed(frame).unwrap();

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::ServerError { code, message }
                if code == "server_error" && message == "stream failed"
        )));
        assert!(parser.finish().is_ok());
    }

    #[test]
    fn truncated_stream_does_not_emit_stream_end() {
        let mut parser = OpenAiResponsesSseParser::new();
        let mut chunks = parser
            .feed(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_truncated\",\"status\":\"in_progress\"}}",
        )
            .unwrap();
        chunks.extend(parser.feed(
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"partial\"}",
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
}
