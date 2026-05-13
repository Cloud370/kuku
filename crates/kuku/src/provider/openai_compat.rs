use reqwest::Client;
use serde_json::{json, Value};

use crate::context::{CanonicalMessage, ContextSource, MessageBlock, Role};

use super::error::{classify_http_error, parse_error, transport_error};
use super::types::{
    ProviderFailure, ProviderRequest, ProviderResponse, ProviderUsage, ResolvedProvider,
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

    if let Some(max_tokens) = request.max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }

    body
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
                messages.extend(history.iter().map(convert_canonical_message));
            }
            ContextSource::Tools(_) => {}
        }
    }

    messages
}

fn convert_canonical_message(message: &CanonicalMessage) -> Value {
    match message.role {
        Role::User => json!({
            "role": "user",
            "content": user_blocks_to_text(&message.blocks),
        }),
        Role::Assistant => {
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
    }
}

fn user_blocks_to_text(blocks: &[MessageBlock]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            MessageBlock::Text(text) => parts.push(text.clone()),
            MessageBlock::ToolResult(result) => parts.push(result.model_content.clone()),
            MessageBlock::ToolUse(_) => {}
        }
    }
    parts.join("\n")
}

fn parse_response(
    body: &str,
    request_id: Option<String>,
) -> Result<ProviderResponse, ProviderFailure> {
    let parsed: Value = serde_json::from_str(body).map_err(|_| parse_error(body))?;

    let usage = parsed
        .get("usage")
        .and_then(Value::as_object)
        .map(|usage| ProviderUsage {
            input_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
            output_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
        })
        .filter(|usage| usage.input_tokens.is_some() || usage.output_tokens.is_some());

    Ok(ProviderResponse {
        assistant_text: parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        stop_reason: parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        provider_request_id: request_id,
        usage,
    })
}
