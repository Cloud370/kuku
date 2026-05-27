use super::http_client;
use serde_json::{json, Value};

use crate::context::{CanonicalMessage, MessageBlock, Role};

use super::chunk::ProviderChunk;
use super::error::{classify_http_error, transport_error};
use super::sse::stream_sse_events;
use super::types::{ProviderFailure, ProviderRequest, ResolvedProvider};

pub(crate) fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

pub(crate) fn render_body(request: &ProviderRequest) -> Value {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.assembly.system_prompt,
    })];
    for message in &request.assembly.prelude_messages {
        messages.extend(convert_user_message(message));
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
    if request.think_level != "off" {
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
                    messages.push(json!({"role": "user", "content": text_parts.join("\n")}));
                    text_parts.clear();
                }
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": result.tool_call_id,
                    "content": result.model_content,
                }));
            }
            MessageBlock::ToolUse(_) | MessageBlock::Thinking(_) => {}
        }
    }

    if !text_parts.is_empty() {
        messages.push(json!({"role": "user", "content": text_parts.join("\n")}));
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
            MessageBlock::ToolUse(_) | MessageBlock::ToolResult(_) | MessageBlock::Thinking(_) => {
                None
            }
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
            MessageBlock::Text(_) | MessageBlock::ToolResult(_) | MessageBlock::Thinking(_) => None,
        })
        .collect::<Vec<_>>();

    let mut msg = json!({"role": "assistant"});
    if !thinking.is_empty() {
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

fn normalize_stop_reason(reason: &str) -> String {
    match reason {
        "tool_calls" => "tool_use".to_string(),
        "stop" => "end_turn".to_string(),
        other => other.to_string(),
    }
}

pub(crate) async fn stream(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<super::ProviderChunkStream, ProviderFailure> {
    let mut body = render_body(request);
    body["stream"] = json!(true);
    body["stream_options"] = json!({"include_usage": true});
    let url = chat_completions_url(&config.base_url);
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
        .or_else(|| response.headers().get("request-id"))
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let mut failure = classify_http_error(status.as_u16(), &body_text);
        failure.provider_request_id = request_id;
        return Err(failure);
    }

    let mut parser = OpenAiCompatSseParser::new();
    Ok(stream_sse_events(response, move |frame| {
        parser.feed(frame);
        parser.take_chunks()
    }))
}

struct OpenAiCompatSseParser {
    chunks: Vec<ProviderChunk>,
    started: bool,
    tool_call_indices: Vec<u64>,
}

impl OpenAiCompatSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            started: false,
            tool_call_indices: Vec::new(),
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

        let data_str = match frame.strip_prefix("data:") {
            Some(s) => s.trim(),
            None => return,
        };

        if data_str == "[DONE]" {
            return;
        }

        let data: Value = match serde_json::from_str(data_str) {
            Ok(v) => v,
            Err(_) => return,
        };

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
            return;
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

        let choices = match data.get("choices").and_then(Value::as_array) {
            Some(c) if !c.is_empty() => c,
            _ => {
                if let Some(usage) = data.get("usage").and_then(Value::as_object) {
                    self.chunks.push(ProviderChunk::StreamUsage {
                        input_tokens: usage
                            .get("prompt_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        output_tokens: usage
                            .get("completion_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_read_input_tokens: usage
                            .get("prompt_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_creation_input_tokens: 0,
                    });
                }
                return;
            }
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
            .and_then(|d| d.get("reasoning_content"))
            .and_then(Value::as_str)
        {
            if !text.is_empty() {
                self.chunks.push(ProviderChunk::ThinkingDelta {
                    text: text.to_string(),
                });
            }
        }

        if let Some(tool_calls) = delta
            .and_then(|d| d.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for tc in tool_calls {
                let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0);
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
            for &idx in &self.tool_call_indices {
                self.chunks
                    .push(ProviderChunk::ContentBlockStop { index: idx });
            }
            self.tool_call_indices.clear();

            self.chunks.push(ProviderChunk::StopReason {
                reason: normalize_stop_reason(reason),
            });
        }
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
        parser.feed(frame);
        let chunks = parser.take_chunks();
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            ProviderChunk::ServerError { code, message } => {
                assert_eq!(code, "server_error");
                assert_eq!(message, "something went wrong");
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }
}
