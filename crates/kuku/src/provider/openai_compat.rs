use reqwest::Client;
use serde_json::{json, Value};

use crate::context::{CanonicalMessage, ContextSource, MessageBlock, Role};

use super::error::{classify_http_error, parse_error, transport_error};
use super::types::{
    ProviderFailure, ProviderRequest, ProviderResponse, ProviderToolCall, ProviderUsage,
    ResolvedProvider,
};

pub(crate) async fn call(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<ProviderResponse, ProviderFailure> {
    let body = render_body(request);
    let url = chat_completions_url(&config.base_url);
    let client = Client::new();

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

pub(crate) fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

pub(crate) fn render_body(request: &ProviderRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": build_messages(&request.assembly.sources),
        "stream": false,
    });
    let tools = build_tools(&request.assembly.sources);

    if let Some(max_tokens) = request.max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    body
}

fn build_tools(sources: &[ContextSource]) -> Vec<Value> {
    sources
        .iter()
        .find_map(|source| match source {
            ContextSource::Tools(schemas) => Some(
                schemas
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
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn build_messages(sources: &[ContextSource]) -> Vec<Value> {
    let mut messages = Vec::new();

    for source in sources {
        match source {
            ContextSource::ProjectInstructions(instructions) => {
                messages.extend(
                    instructions.iter().map(
                        |instruction| json!({"role": "system", "content": instruction.content}),
                    ),
                );
            }
            ContextSource::GlobalMemory(memory) | ContextSource::ProjectMemory(memory) => {
                messages.push(json!({"role": "system", "content": memory.content}));
            }
            ContextSource::History(history) => {
                for message in history {
                    match message.role {
                        Role::User => messages.extend(convert_user_message(message)),
                        Role::Assistant => messages.push(convert_assistant_message(message)),
                    }
                }
            }
            ContextSource::Tools(_) => {}
        }
    }

    messages
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
            MessageBlock::ToolUse(_) => {}
        }
    }

    if !text_parts.is_empty() {
        messages.push(json!({"role": "user", "content": text_parts.join("\n")}));
    }

    messages
}

fn convert_assistant_message(message: &CanonicalMessage) -> Value {
    let text = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(text) => Some(text.as_str()),
            MessageBlock::ToolUse(_) | MessageBlock::ToolResult(_) => None,
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
            MessageBlock::Text(_) | MessageBlock::ToolResult(_) => None,
        })
        .collect::<Vec<_>>();

    if tool_calls.is_empty() {
        json!({"role": "assistant", "content": text})
    } else {
        json!({"role": "assistant", "content": text, "tool_calls": tool_calls})
    }
}

fn normalize_stop_reason(reason: &str) -> String {
    match reason {
        "tool_calls" => "tool_use".to_string(),
        "stop" => "end_turn".to_string(),
        other => other.to_string(),
    }
}

fn parse_response(
    body: &str,
    request_id: Option<String>,
) -> Result<ProviderResponse, ProviderFailure> {
    let parsed: Value = serde_json::from_str(body).map_err(|_| parse_error(body))?;

    let choice = parsed
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let message = choice.and_then(|choice| choice.get("message"));
    let tool_call_request_id = request_id.as_deref().unwrap_or("unknown");

    let tool_calls = message
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .map(|(index, call)| {
                    let function = call.get("function");
                    let args = function
                        .and_then(|function| function.get("arguments"))
                        .and_then(Value::as_str)
                        .and_then(|args| serde_json::from_str(args).ok())
                        .unwrap_or_else(|| json!({}));
                    ProviderToolCall {
                        id: call
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| format!("tc_{tool_call_request_id}_{index}")),
                        name: function
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        args,
                        index: index as u64,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let usage = parsed
        .get("usage")
        .and_then(Value::as_object)
        .map(|usage| ProviderUsage {
            input_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
            output_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
        })
        .filter(|usage| usage.input_tokens.is_some() || usage.output_tokens.is_some());

    Ok(ProviderResponse {
        assistant_text: message
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        stop_reason: choice
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .map(normalize_stop_reason),
        provider_request_id: request_id,
        usage,
        tool_calls,
    })
}
