use serde_json::{Value, json};

use crate::config::ProviderFormat;
use crate::context::{CanonicalMessage, MessageBlock, Role};
use crate::execution::ModelImage;

use super::super::media::{ImageTargets, ToolImageTarget};
use super::super::replay::native_replay;
use super::super::types::ProviderRequest;

fn convert_to_input_items(
    message: &CanonicalMessage,
    input_offset: usize,
    mut images: Option<&mut ImageTargets>,
) -> Vec<Value> {
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
                    MessageBlock::InputImage(image) => {
                        if let Some(images) = images.as_deref_mut() {
                            images
                                .user
                                .push((input_offset + items.len(), image.clone()));
                            if text_parts.is_empty() {
                                text_parts.push(String::new());
                            }
                        }
                    }
                    MessageBlock::ToolResult(result) => {
                        if !text_parts.is_empty() {
                            items.push(json!({
                                "type": "message",
                                "role": "user",
                                "content": input_text_parts(&text_parts),
                            }));
                            text_parts.clear();
                        }
                        if let Some(images) = images.as_deref_mut() {
                            if let Some(image) = result
                                .structured
                                .as_ref()
                                .and_then(ModelImage::from_tool_result)
                            {
                                images.tools.push((
                                    ToolImageTarget::Item(input_offset + items.len()),
                                    image,
                                ));
                            }
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
                    | MessageBlock::Thinking(_)
                    | MessageBlock::InputImage(_) => None,
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
                MessageBlock::Text(_) | MessageBlock::ToolResult(_) | MessageBlock::Thinking(_) | MessageBlock::InputImage(_) | MessageBlock::ProviderReplay { .. } => {
                    None
                }
            }));

            items
        }
    }
}

fn input_text_parts(text_parts: &[String]) -> Value {
    json!(
        text_parts
            .iter()
            .map(|text| json!({"type": "input_text", "text": text}))
            .collect::<Vec<_>>()
    )
}

#[cfg(test)]
pub(crate) fn render_body(request: &ProviderRequest<'_>) -> crate::error::Result<Value> {
    render_body_with_images(request, None)
}

pub(crate) fn render_body_with_images(
    request: &ProviderRequest<'_>,
    mut images: Option<&mut ImageTargets>,
) -> crate::error::Result<Value> {
    let mut input_items = Vec::new();

    if !request.assembly.system_prompt.is_empty() {
        input_items.push(json!({
            "type": "message",
            "role": "developer",
            "content": [{"type":"input_text", "text": request.assembly.system_prompt}],
        }));
    }
    for message in &request.assembly.prelude_messages {
        let offset = input_items.len();
        input_items.extend(convert_to_input_items(
            message,
            offset,
            images.as_deref_mut(),
        ));
    }
    if let Some(content) = request.render_handoff()? {
        input_items.push(json!({
            "role": "user",
            "content": content,
        }));
    }
    for (index, message) in request.assembly.history.iter().enumerate() {
        let offset = input_items.len();
        if let Some(images) = images.as_deref_mut()
            && images.current_user_index == Some(index)
        {
            images.current_input_offset = Some(offset);
        }
        input_items.extend(convert_to_input_items(
            message,
            offset,
            images.as_deref_mut(),
        ));
    }

    let mut body = json!({
        "model": request.model,
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
        body["tools"] = json!(
            request
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
                .collect::<Vec<_>>()
        );
    }

    Ok(body)
}
