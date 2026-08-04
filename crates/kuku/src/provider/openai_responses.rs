use super::http_client;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use wreq::header::{HeaderMap, HeaderValue};

use crate::context::{CanonicalMessage, MessageBlock, Role};

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

struct OpenAiResponsesSseParser {
    chunks: Vec<ProviderChunk>,
    started: bool,
    completed: bool,
}

impl OpenAiResponsesSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            started: false,
            completed: false,
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
                let item = match data.get("item") {
                    Some(i) => i,
                    None => return Ok(self.take_chunks()),
                };
                if let Some("function_call") = item.get("type").and_then(Value::as_str) {
                    let index = data
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
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
                let index = data
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    if !delta.is_empty() {
                        self.chunks.push(ProviderChunk::ToolCallArgDelta {
                            index,
                            fragment: delta.to_string(),
                        });
                    }
                }
            }
            "response.output_item.done" => {
                let index = data
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.chunks.push(ProviderChunk::ContentBlockStop { index });
            }
            "response.completed" => {
                if let Some(resp) = data.get("response") {
                    self.push_usage(resp);
                    if let Some(status) = resp.get("status").and_then(Value::as_str) {
                        let reason = match status {
                            "completed" => "end_turn",
                            other => other,
                        };
                        self.chunks.push(ProviderChunk::StopReason {
                            reason: reason.to_string(),
                        });
                    }
                }
                self.completed = true;
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "response.incomplete" => {
                if let Some(resp) = data.get("response") {
                    self.push_usage(resp);
                    if let Some(reason) = resp
                        .get("incomplete_details")
                        .and_then(|details| details.get("reason"))
                        .and_then(Value::as_str)
                    {
                        self.chunks.push(ProviderChunk::StopReason {
                            reason: reason.to_string(),
                        });
                    }
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
            let input_tokens_total = usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cache_read_input_tokens = usage
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            self.chunks.push(ProviderChunk::StreamUsage {
                input_tokens: input_tokens_total.saturating_sub(cache_read_input_tokens),
                output_tokens: usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_read_input_tokens,
                cache_creation_input_tokens: 0,
            });
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
                input_tokens: 50,
                output_tokens: 30,
                cache_read_input_tokens: 70,
                cache_creation_input_tokens: 0
            }
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
                input_tokens: 50,
                output_tokens: 30,
                cache_read_input_tokens: 70,
                cache_creation_input_tokens: 0
            }
        )));
        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::StopReason { reason } if reason == "max_tokens"
        )));
        assert!(chunks
            .last()
            .is_some_and(|chunk| matches!(chunk, ProviderChunk::StreamEnd)));
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
