use reqwest::Client;
use serde_json::{json, Value};

use crate::context::{CanonicalMessage, MessageBlock, Role};

use super::chunk::ProviderChunk;
use super::error::{classify_http_error, parse_error, transport_error};
use super::types::{
    ProviderFailure, ProviderRequest, ProviderResponse, ProviderToolCall, ProviderUsage,
    ResolvedProvider,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";

pub(crate) async fn call(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<ProviderResponse, ProviderFailure> {
    let body = render_body(request);
    let url = messages_url(&config.base_url);
    let client = Client::new();

    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-api-key", config.api_key.expose())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|error| transport_error(&error))?;

    let status = response.status();
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);

    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let mut failure = classify_http_error(status.as_u16(), &body_text);
        failure.provider_request_id = request_id;
        return Err(failure);
    }

    let body_text = response.text().await.map_err(|error| {
        let mut failure = transport_error(&error);
        failure.provider_request_id = request_id.clone();
        failure
    })?;

    parse_response(&body_text, request_id)
}

pub(crate) fn messages_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

pub(crate) fn render_body(request: &ProviderRequest) -> Value {
    let mut messages = request
        .assembly
        .prelude_messages
        .iter()
        .map(convert_canonical_message)
        .collect::<Vec<_>>();
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
        "max_tokens": request.max_output_tokens.unwrap_or(4096),
        "stream": false,
        "system": request.assembly.system_prompt,
    });

    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
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

fn convert_canonical_message(message: &CanonicalMessage) -> Value {
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
                    })),
                    MessageBlock::ToolUse(_) => None,
                })
                .collect::<Vec<_>>();

            json!({"role": "user", "content": content})
        }
        Role::Assistant => {
            let content = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Text(text) => Some(json!({"type": "text", "text": text})),
                    MessageBlock::ToolUse(tool_use) => Some(json!({
                        "type": "tool_use",
                        "id": tool_use.id,
                        "name": tool_use.name,
                        "input": tool_use.args,
                    })),
                    MessageBlock::ToolResult(_) => None,
                })
                .collect::<Vec<_>>();

            json!({"role": "assistant", "content": content})
        }
    }
}

fn parse_response(
    body: &str,
    request_id: Option<String>,
) -> Result<ProviderResponse, ProviderFailure> {
    let parsed: Value = serde_json::from_str(body).map_err(|_| parse_error(body))?;

    let assistant_text = parsed
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .map(|block| {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    let tool_calls = parsed
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
                .enumerate()
                .map(|(index, block)| ProviderToolCall {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    args: block.get("input").cloned().unwrap_or_else(|| json!({})),
                    index: index as u64,
                })
                .collect()
        })
        .unwrap_or_default();

    let usage = parsed
        .get("usage")
        .and_then(Value::as_object)
        .map(|usage| ProviderUsage {
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
        })
        .filter(|usage| usage.input_tokens.is_some() || usage.output_tokens.is_some());

    Ok(ProviderResponse {
        assistant_text,
        stop_reason: parsed
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        provider_request_id: request_id.or_else(|| {
            parsed
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        }),
        usage,
        tool_calls,
    })
}

pub(crate) async fn stream(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<super::ProviderChunkStream, ProviderFailure> {
    let mut body = render_body(request);
    body["stream"] = json!(true);
    let url = messages_url(&config.base_url);
    let client = Client::new();

    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-api-key", config.api_key.expose())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|error| transport_error(&error))?;

    let status = response.status();
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let mut failure = classify_http_error(status.as_u16(), &body_text);
        failure.provider_request_id = request_id;
        return Err(failure);
    }

    let body_text = response
        .text()
        .await
        .map_err(|error| transport_error(&error))?;
    let chunks = parse_anthropic_sse(&body_text);
    Ok(Box::pin(tokio_stream::iter(chunks.into_iter().map(Ok))))
}

fn parse_anthropic_sse(body: &str) -> Vec<ProviderChunk> {
    let mut chunks = Vec::new();
    let mut text_buffers: Vec<String> = Vec::new();
    let mut tool_arg_buffers: Vec<(u64, String)> = Vec::new(); // (index, buffer)

    for frame in body.split("\n\n") {
        let frame = frame.trim();
        if frame.is_empty() {
            continue;
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

        if data_str.is_empty() || event_type == "ping" {
            continue;
        }

        let data: Value = match serde_json::from_str(data_str) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match event_type {
            "message_start" => {
                if let Some(msg) = data.get("message") {
                    let rid = msg
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let mdl = msg
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    chunks.push(ProviderChunk::StreamStart {
                        request_id: rid,
                        model: mdl,
                    });
                }
            }
            "content_block_start" => {
                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(block) = data.get("content_block") {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            while text_buffers.len() <= index as usize {
                                text_buffers.push(String::new());
                            }
                        }
                        Some("tool_use") => {
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
                            chunks.push(ProviderChunk::ToolCallStart {
                                index,
                                id: id.clone(),
                                name: name.clone(),
                            });
                            tool_arg_buffers.push((index, String::new()));
                        }
                        _ => {}
                    }
                }
            }
            "content_block_delta" => {
                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(delta) = data.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = delta
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if let Some(buf) = text_buffers.get_mut(index as usize) {
                                buf.push_str(&text);
                            }
                            chunks.push(ProviderChunk::TextDelta { text });
                        }
                        Some("input_json_delta") => {
                            let fragment = delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if let Some((_, buf)) =
                                tool_arg_buffers.iter_mut().find(|(i, _)| *i == index)
                            {
                                buf.push_str(&fragment);
                            }
                            chunks.push(ProviderChunk::ToolCallArgDelta { index, fragment });
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                chunks.push(ProviderChunk::ContentBlockStop { index });
            }
            "message_delta" => {
                if let Some(delta) = data.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                        chunks.push(ProviderChunk::StopReason {
                            reason: reason.to_string(),
                        });
                    }
                }
                if let Some(usage) = data.get("usage") {
                    chunks.push(ProviderChunk::StreamUsage {
                        input_tokens: usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        output_tokens: usage
                            .get("output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_read_input_tokens: usage
                            .get("cache_read_input_tokens")
                            .and_then(Value::as_u64),
                        cache_creation_input_tokens: usage
                            .get("cache_creation_input_tokens")
                            .and_then(Value::as_u64),
                    });
                }
            }
            "message_stop" => {
                chunks.push(ProviderChunk::StreamEnd);
            }
            "error" => {
                // Skip in-stream errors for now; the stream will end without StreamEnd.
            }
            _ => {}
        }
    }

    // Ensure StreamEnd is always present
    if !chunks.iter().any(|c| matches!(c, ProviderChunk::StreamEnd)) {
        chunks.push(ProviderChunk::StreamEnd);
    }

    chunks
}
