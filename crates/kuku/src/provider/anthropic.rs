use reqwest::Client;
use serde_json::{json, Value};

use crate::context::{CanonicalMessage, ContextSource, MessageBlock, Role};

use super::error::{classify_http_error, parse_error, transport_error};
use super::types::{
    ProviderFailure, ProviderRequest, ProviderResponse, ProviderUsage, ResolvedProvider,
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
    let (messages, system) = build_messages(&request.assembly.sources);
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_output_tokens.unwrap_or(4096),
        "stream": false,
    });

    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(system_text) = system {
        body["system"] = json!(system_text);
    }

    body
}

fn build_messages(sources: &[ContextSource]) -> (Vec<Value>, Option<String>) {
    let mut messages = Vec::new();
    let mut system_parts = Vec::new();

    for source in sources {
        match source {
            ContextSource::ProjectInstructions(instructions) => {
                system_parts.extend(
                    instructions
                        .iter()
                        .map(|instruction| instruction.content.clone()),
                );
            }
            ContextSource::GlobalMemory(memory) | ContextSource::ProjectMemory(memory) => {
                system_parts.push(memory.content.clone());
            }
            ContextSource::History(history) => {
                messages.extend(history.iter().map(convert_canonical_message));
            }
            ContextSource::Tools(_) => {}
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    (messages, system)
}

fn convert_canonical_message(message: &CanonicalMessage) -> Value {
    match message.role {
        Role::User => json!({
            "role": "user",
            "content": blocks_to_text(&message.blocks),
        }),
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

fn blocks_to_text(blocks: &[MessageBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(text) => Some(text.as_str()),
            MessageBlock::ToolResult(result) => Some(result.model_content.as_str()),
            MessageBlock::ToolUse(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    })
}
