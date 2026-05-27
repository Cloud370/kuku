use super::http_client;
use serde_json::{json, Value};

use crate::context::{CanonicalMessage, MessageBlock, Role};

use super::chunk::ProviderChunk;
use super::error::{classify_http_error, transport_error};
use super::sse::stream_sse_events;
use super::types::{ProviderFailure, ProviderRequest, ResolvedProvider};

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
                                "content": text_parts.join("\n"),
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
                    "content": text_parts.join("\n"),
                }));
            }

            items
        }
        Role::Assistant => {
            let text = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            if text.is_empty() {
                return Vec::new();
            }

            vec![json!({
                "type": "message",
                "role": "assistant",
                "content": text,
            })]
        }
    }
}

pub(crate) fn render_body(request: &ProviderRequest) -> Value {
    let mut input_items = Vec::new();

    for message in &request.assembly.prelude_messages {
        input_items.extend(convert_to_input_items(message));
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
    if request.think_level != "off" {
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
    request: &ProviderRequest,
) -> Result<super::ProviderChunkStream, ProviderFailure> {
    let mut body = render_body(request);
    body["stream"] = json!(true);
    let url = responses_url(&config.base_url);
    let client = http_client::api_client();

    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("Bearer {}", config.api_key.expose()),
        )
        .json(&body)
        .send()
        .await
        .map_err(|error| transport_error(&error))?;

    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let mut failure = classify_http_error(status.as_u16(), &body_text);
        failure.provider_request_id = request_id;
        return Err(failure);
    }

    let mut parser = OpenAiResponsesSseParser::new();
    Ok(stream_sse_events(response, move |frame| {
        parser.feed(frame);
        parser.take_chunks()
    }))
}

struct OpenAiResponsesSseParser {
    chunks: Vec<ProviderChunk>,
    started: bool,
}

impl OpenAiResponsesSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            started: false,
        }
    }

    fn feed(&mut self, frame: &str) {
        if frame.is_empty() {
            if !self
                .chunks
                .iter()
                .any(|c| matches!(c, ProviderChunk::StreamEnd))
            {
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            return;
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
            return;
        }

        let data: Value = match serde_json::from_str(data_str) {
            Ok(v) => v,
            Err(_) => return,
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
                    None => return,
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
                    if let Some(usage) = resp.get("usage") {
                        self.chunks.push(ProviderChunk::StreamUsage {
                            input_tokens: usage
                                .get("input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            output_tokens: usage
                                .get("output_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            cache_read_input_tokens: usage
                                .get("input_tokens_details")
                                .and_then(|d| d.get("cached_tokens"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            cache_creation_input_tokens: 0,
                        });
                    }
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
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "response.failed" | "error" => {
                let code = event_type.to_string();
                let message = data
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        data.get("response")
                            .and_then(|r| r.get("status_details"))
                            .and_then(|d| d.as_str())
                    })
                    .unwrap_or("server error")
                    .to_string();
                self.chunks
                    .push(ProviderChunk::ServerError { code, message });
            }
            _ => {}
        }
    }

    fn take_chunks(&mut self) -> Vec<ProviderChunk> {
        std::mem::take(&mut self.chunks)
    }
}

#[allow(dead_code)]
pub(crate) fn parse_responses_sse(body: &str) -> Vec<ProviderChunk> {
    let mut parser = OpenAiResponsesSseParser::new();
    for frame in body.split("\n\n") {
        parser.feed(frame.trim());
    }
    parser.feed("");
    parser.take_chunks()
}
