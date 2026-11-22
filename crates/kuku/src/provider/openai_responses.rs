use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::config::ProviderFormat;
use crate::event::ModelStopReason;

use super::chunk::ProviderChunk;
use super::replay::{validate_replay_size, ReplayBudget, MAX_REPLAY_ITEMS};
use super::report::ReportAccumulator;
use super::sse::{parse_sse_frame, stream_sse_events};
use super::transport::{send_prepared, TransportResponse};
use super::types::{PreparedProviderRequest, ProviderFailure, ProviderRequest, ResolvedProvider};

#[path = "openai_responses/media.rs"]
mod media;
#[path = "openai_responses/reasoning.rs"]
mod reasoning;
#[path = "openai_responses/render.rs"]
mod render;

pub(super) use media::{attach_tool_images, redact_tool_images};
use reasoning::{PublishedReasoning, ReasoningText};
#[cfg(test)]
pub(crate) use render::render_body;
pub(crate) use render::render_body_with_images;

const TRUNCATED_STREAM_MESSAGE: &str = "provider stream ended before response.completed";

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
    let parser = Arc::new(Mutex::new(OpenAiResponsesSseParser::new()));
    let frame_parser = Arc::clone(&parser);
    let eof_parser = Arc::clone(&parser);
    Ok(stream_sse_events(
        response,
        trace,
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
    chunks: Vec<ProviderChunk>,
    started: bool,
    terminal_event: Option<ResponseTerminalEvent>,
    output_item_types: BTreeMap<u64, String>,
    item_id_indices: BTreeMap<String, u64>,
    function_calls: BTreeMap<u64, ResponseFunctionCall>,
    invalid_function_calls: bool,
    replay_items: BTreeMap<u64, Value>,
    replay_budget: ReplayBudget,
    reasoning: PublishedReasoning,
    reports: ReportAccumulator,
}

impl OpenAiResponsesSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            started: false,
            terminal_event: None,
            output_item_types: BTreeMap::new(),
            item_id_indices: BTreeMap::new(),
            function_calls: BTreeMap::new(),
            invalid_function_calls: false,
            replay_items: BTreeMap::new(),
            replay_budget: ReplayBudget::default(),
            reasoning: PublishedReasoning::default(),
            reports: ReportAccumulator::default(),
        }
    }

    fn publish_reasoning(&mut self, event: &Value, kind: ReasoningText, complete: bool) {
        let Some(text) = event[if complete { "text" } else { "delta" }].as_str() else {
            return;
        };
        let item_index = event["item_id"]
            .as_str()
            .and_then(|id| self.item_id_indices.get(id).copied());
        let index = event["output_index"].as_u64().or(item_index);
        if let Some(index) = index {
            if item_index.is_none_or(|owner| owner == index)
                && self
                    .output_item_types
                    .get(&index)
                    .is_none_or(|item| item == "reasoning")
            {
                self.reasoning.publish(
                    index,
                    kind.part_index(event),
                    kind,
                    text,
                    complete,
                    &mut self.chunks,
                );
            }
        } else if !text.is_empty() {
            self.chunks.push(ProviderChunk::Reasoning);
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
            super::prepare::invalid_response("provider emitted malformed JSON event")
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
                    self.reasoning.announce(index, &mut self.chunks);
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
            "response.reasoning_text.delta" | "response.reasoning_text.done" => {
                self.publish_reasoning(&data, ReasoningText::Text, event_type.ends_with(".done"));
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_summary_text.done" => {
                self.publish_reasoning(
                    &data,
                    ReasoningText::Summary,
                    event_type.ends_with(".done"),
                );
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
                    self.reasoning.item(index, item, &mut self.chunks);
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
                                self.reasoning.item(index as u64, item, &mut self.chunks);
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
                let error = if event_type == "error" {
                    Some(&data)
                } else {
                    data.get("response")
                        .and_then(|response| response.get("error"))
                };
                let code = error
                    .and_then(|value| {
                        super::error::select_provider_error_code(
                            value.get("code").and_then(Value::as_str),
                            value.get("type").and_then(Value::as_str),
                        )
                    })
                    .unwrap_or(event_type)
                    .to_string();
                let message = error
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("server error")
                    .to_string();
                self.chunks
                    .push(ProviderChunk::ServerError { code, message });
                if let Some(response) = data.get("response") {
                    self.push_usage(response)?;
                }
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
        let known_type = self.output_item_types.get(&index).map(String::as_str);
        if known_type != Some(item_type) && !(item_type == "reasoning" && known_type.is_none()) {
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
#[path = "openai_responses/tests.rs"]
mod tests;
