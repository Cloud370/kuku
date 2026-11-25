use std::collections::HashSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::execution::{InputImage, MAX_MODEL_IMAGE_BYTES, ModelImage};
use crate::provider::prepare::invalid_request;
use crate::provider::types::{ProviderFailure, ProviderFailureKind, ProviderKind};

const MAX_IMAGES_PER_REQUEST: usize = 4;

#[derive(Default)]
pub(crate) struct ImageTargets {
    pub(crate) tools: Vec<(ToolImageTarget, ModelImage)>,
    pub(crate) user: Vec<(usize, InputImage)>,
    pub(crate) current_user_index: Option<usize>,
    pub(crate) current_input_offset: Option<usize>,
}

#[derive(Clone, Copy)]
pub(crate) enum ToolImageTarget {
    Item(usize),
    MessageBlock { message: usize, block: usize },
}

#[derive(Clone, Copy)]
pub(super) enum ImageWire {
    ChatCompletions,
    Responses,
    Messages,
}

impl ImageWire {
    pub(super) fn for_kind(kind: ProviderKind) -> Self {
        match kind {
            ProviderKind::OpenAiCompatible => Self::ChatCompletions,
            ProviderKind::OpenAiResponses => Self::Responses,
            ProviderKind::Anthropic => Self::Messages,
        }
    }

    fn items(self) -> &'static str {
        match self {
            Self::ChatCompletions | Self::Messages => "messages",
            Self::Responses => "input",
        }
    }

    fn image_part(self, mime: &str) -> Value {
        match self {
            Self::ChatCompletions => json!({
                "type": "image_url", "image_url": {"url": format!("data:{mime};base64,")}
            }),
            Self::Responses => json!({
                "type": "input_image", "image_url": format!("data:{mime};base64,")
            }),
            Self::Messages => json!({
                "type": "image",
                "source": {"type": "base64", "media_type": mime, "data": ""}
            }),
        }
    }

    fn image_data_path(self, part: &Value) -> Option<&'static str> {
        match part["type"].as_str() {
            Some("image_url") if matches!(self, Self::ChatCompletions) => Some("/image_url/url"),
            Some("input_image") if matches!(self, Self::Responses) => Some("/image_url"),
            Some("image")
                if matches!(self, Self::Messages) && part["source"]["type"] == "base64" =>
            {
                Some("/source/data")
            }
            _ => None,
        }
    }

    fn inline_image_data(self, part: &Value) -> Option<(&'static str, &str)> {
        let path = self.image_data_path(part)?;
        let encoded = part.pointer(path)?.as_str()?;
        (matches!(self, Self::Messages) || encoded.starts_with("data:")).then_some((path, encoded))
    }

    fn content_field(self, item: &Value) -> Option<&'static str> {
        match (self, item["type"].as_str()) {
            (Self::Responses, Some("function_call_output")) => Some("output"),
            (Self::Responses, Some("message")) | (Self::ChatCompletions | Self::Messages, _) => {
                Some("content")
            }
            _ => None,
        }
    }

    fn has_nested_tool_content(self, part: &Value) -> bool {
        matches!(self, Self::Messages) && part["type"] == "tool_result"
    }

    fn encode(self, part: &mut Value, bytes: &[u8]) {
        let path = self
            .image_data_path(part)
            .expect("native inline image part");
        let data = part.pointer_mut(path).expect("inline image data field");
        let mut encoded = data.as_str().expect("base64 data string").to_owned();
        STANDARD.encode_string(bytes, &mut encoded);
        *data = Value::String(encoded);
    }
}

pub(super) fn attach_tool_images(
    wire: ImageWire,
    body: &mut Value,
    images: Vec<(ToolImageTarget, ModelImage)>,
    limit: usize,
    load: &mut dyn FnMut(&ModelImage) -> crate::Result<Vec<u8>>,
    chat_continuation: Option<&str>,
) {
    if images.is_empty() {
        return;
    }
    let mut available = limit.saturating_sub(json_bytes(body));
    let mut attached = HashSet::new();
    let mut chat_insertions = Vec::new();
    for (target, image) in images.into_iter().rev() {
        if attached.len() == MAX_IMAGES_PER_REQUEST {
            break;
        }
        if attached.contains(&image.output.id) || image.output.bytes > MAX_MODEL_IMAGE_BYTES as u64
        {
            continue;
        }
        let (index, item) = match target {
            ToolImageTarget::Item(i) => (i, &mut body[wire.items()][i]),
            ToolImageTarget::MessageBlock { message, block } => {
                (message, &mut body["messages"][message]["content"][block])
            }
        };
        let part = wire.image_part(image.mime.as_str());
        let (mut value, additional) = match wire {
            ImageWire::Responses => {
                let parts = json!([{"type": "input_text", "text": item["output"]}, part]);
                let delta = json_bytes(&parts) - json_bytes(&item["output"]);
                (parts, delta)
            }
            ImageWire::Messages => {
                let parts = json!([{"type": "text", "text": item["content"]}, part]);
                let delta = json_bytes(&parts) - json_bytes(&item["content"]);
                (parts, delta)
            }
            ImageWire::ChatCompletions => {
                let content = format!(
                    "Image from view_image tool call {} (tool output, not a user attachment).",
                    item["tool_call_id"]
                );
                let mut content = vec![json!({"type": "text", "text": content}), part];
                if let Some(note) = chat_continuation.filter(|note| !note.is_empty()) {
                    content.push(json!({"type": "text", "text": note}));
                }
                let message = json!({
                    "role": "user",
                    "content": content,
                });
                let delta = json_bytes(&message) + 1; // comma in the messages array
                (message, delta)
            }
        };
        let additional = additional + (image.output.bytes as usize).div_ceil(3) * 4;
        if additional > available {
            continue;
        }
        let Ok(bytes) = load(&image) else {
            continue;
        };
        if bytes.len() as u64 != image.output.bytes {
            continue;
        }
        match wire {
            ImageWire::Responses => {
                wire.encode(&mut value[1], &bytes);
                item["output"] = value;
            }
            ImageWire::Messages => {
                wire.encode(&mut value[1], &bytes);
                item["content"] = value;
            }
            ImageWire::ChatCompletions => {
                wire.encode(&mut value["content"][1], &bytes);
                // Chat requires every tool result to follow its assistant call. Its
                // tool role accepts text only, so put images after the whole batch.
                let items = body["messages"].as_array().expect("Chat messages array");
                let end = (index + 1..items.len())
                    .find(|&next| items[next]["role"] != "tool")
                    .unwrap_or(items.len());
                chat_insertions.push((end, value));
            }
        }
        attached.insert(image.output.id);
        available -= additional;
    }
    if !chat_insertions.is_empty() {
        let items = body["messages"]
            .as_array_mut()
            .expect("Chat messages array");
        // Targets are visited newest first. Insert in that order so original
        // offsets remain valid and images of the same batch retain call order.
        for (index, message) in chat_insertions {
            items.insert(index, message);
        }
    }
}

pub(super) fn attach_user_images(
    wire: ImageWire,
    body: &mut Value,
    images: Vec<(usize, InputImage)>,
    current_offset: Option<usize>,
    limit: usize,
    load: &mut dyn FnMut(&InputImage) -> crate::error::Result<Vec<u8>>,
) -> Result<(), ProviderFailure> {
    if images.is_empty() {
        return Ok(());
    }
    let mut available = limit.saturating_sub(json_bytes(body));
    for batch in images.chunk_by(|(left, _), (right, _)| left == right).rev() {
        let index = batch[0].0;
        let required = current_offset.is_some_and(|offset| index >= offset);
        let content = &mut body[wire.items()][index]["content"];
        let mut parts = match wire {
            ImageWire::Responses | ImageWire::Messages => {
                content.as_array().expect("native content array").clone()
            }
            ImageWire::ChatCompletions => {
                let text = content.as_str().expect("Chat user content string");
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![json!({"type": "text", "text": text})]
                }
            }
        };
        let first_image = parts.len();
        for (_, image) in batch {
            parts.push(wire.image_part(image.media_type()));
        }
        let mut new_content = Value::Array(parts);
        let encoded: usize = batch
            .iter()
            .map(|(_, image)| image.byte_len().div_ceil(3) * 4)
            .sum();
        let additional = json_bytes(&new_content) + encoded - json_bytes(content);
        if additional > available {
            if required {
                return Err(ProviderFailure {
                    kind: ProviderFailureKind::ContextTooLarge,
                    ..invalid_request("image attachments exceed the remaining request byte limit")
                });
            }
            continue;
        }
        let loaded = batch
            .iter()
            .enumerate()
            .try_for_each(|(index, (_, image))| {
                let bytes = load(image)?;
                wire.encode(&mut new_content[first_image + index], &bytes);
                Ok::<_, crate::Error>(())
            });
        match loaded {
            Ok(()) => {}
            Err(error) if required => {
                return Err(invalid_request(&error.to_string()));
            }
            Err(_) => continue,
        }
        *content = new_content;
        available -= additional;
    }
    Ok(())
}

pub(super) fn has_inline_images(wire: ImageWire, body: &Value) -> bool {
    fn has_inline_parts(wire: ImageWire, parts: &[Value]) -> bool {
        parts.iter().any(|part| {
            wire.inline_image_data(part).is_some()
                || (wire.has_nested_tool_content(part)
                    && part["content"]
                        .as_array()
                        .is_some_and(|parts| has_inline_parts(wire, parts)))
        })
    }

    body[wire.items()].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            wire.content_field(item)
                .and_then(|field| item[field].as_array())
                .is_some_and(|parts| has_inline_parts(wire, parts))
        })
    })
}

pub(super) fn redact_images(wire: ImageWire, body: &mut Value) {
    let Some(input) = body.get_mut(wire.items()).and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        if let Some(field) = wire.content_field(item)
            && let Some(parts) = item.get_mut(field).and_then(Value::as_array_mut)
        {
            redact_parts(wire, parts);
        }
    }
}

fn redact_parts(wire: ImageWire, parts: &mut [Value]) {
    for part in parts {
        if wire.has_nested_tool_content(part)
            && let Some(content) = part["content"].as_array_mut()
        {
            redact_parts(wire, content);
        }
        if let Some((path, encoded)) = wire.inline_image_data(part) {
            let bytes = encoded.len();
            *part.pointer_mut(path).expect("inline image data field") =
                json!(format!("<omitted image data, {bytes} bytes>"));
        }
    }
}

fn json_bytes(value: &Value) -> usize {
    serde_json::to_vec(value).expect("serializable JSON").len()
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::json;

    use super::{
        ImageWire, ToolImageTarget, attach_tool_images, attach_user_images, has_inline_images,
        json_bytes, redact_images,
    };
    use crate::execution::{ImageMime, InputImage, ModelImage, OutputReference};

    #[test]
    fn newest_image_uses_the_exact_remaining_request_budget() {
        let image = ModelImage {
            mime: ImageMime::Png,
            output: OutputReference {
                id: "image".into(),
                offset: 0,
                bytes: 4,
            },
        };
        let body = json!({"input": [
            {"type":"function_call_output", "call_id":"older", "output":"first"},
            {"type":"function_call_output", "call_id":"newer", "output":"newer \"quoted\""},
        ]});
        let candidates = vec![
            (ToolImageTarget::Item(0), image.clone()),
            (ToolImageTarget::Item(1), image),
        ];
        let mut preview = body.clone();
        attach_tool_images(
            ImageWire::Responses,
            &mut preview,
            candidates.clone(),
            usize::MAX,
            &mut |_| Ok(vec![0; 4]),
            None,
        );
        let exact = json_bytes(&preview);
        assert_eq!(preview["input"][0]["output"], "first");
        assert_eq!(
            preview["input"][1]["output"][1]["image_url"],
            "data:image/png;base64,AAAAAA=="
        );

        let mut limited = body.clone();
        attach_tool_images(
            ImageWire::Responses,
            &mut limited,
            candidates.clone(),
            exact,
            &mut |_| Ok(vec![0; 4]),
            None,
        );
        assert_eq!(limited, preview);
        assert!(has_inline_images(ImageWire::Responses, &limited));
        redact_images(ImageWire::Responses, &mut limited);
        assert!(!limited.to_string().contains("data:image/png"));
        assert_eq!(limited["input"][1]["output"][1]["type"], "input_image");

        let mut recent_only = body.clone();
        attach_tool_images(
            ImageWire::Responses,
            &mut recent_only,
            candidates.into_iter().skip(1).collect(),
            exact - 1,
            &mut |_| panic!("image above the byte limit must never load"),
            None,
        );
        assert_eq!(recent_only, body);
    }

    #[test]
    fn chat_attachment_budget_includes_content_array_and_redacts_media() {
        const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg==";
        let wire = ImageWire::ChatCompletions;
        let png = STANDARD.decode(PNG).unwrap();
        let image = InputImage::from_bytes(&png).unwrap();
        let body = json!({"messages": [
            {"role": "system", "content": "fixture"},
            {"role": "user", "content": "read this \"image\""},
        ]});
        let mut expected = body.clone();
        expected["messages"][1]["content"] = json!([
            {"type": "text", "text": "read this \"image\""},
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{PNG}")}},
        ]);
        let exact = json_bytes(&expected);
        let mut fitted = body.clone();
        attach_user_images(
            wire,
            &mut fitted,
            vec![(1, image.clone())],
            Some(1),
            exact,
            &mut |_| Ok(png.clone()),
        )
        .unwrap();
        assert_eq!(fitted, expected);
        assert!(has_inline_images(wire, &fitted));
        redact_images(wire, &mut fitted);
        assert!(!fitted.to_string().contains("data:image/"));
        assert_eq!(fitted["messages"][1]["content"][1]["type"], "image_url");
        let mut unfitted = body.clone();
        let fail = attach_user_images(
            wire,
            &mut unfitted,
            vec![(1, image)],
            Some(1),
            exact - 1,
            &mut |_| panic!("over-budget image must not load"),
        )
        .unwrap_err();
        assert_eq!(
            fail.kind,
            crate::provider::types::ProviderFailureKind::ContextTooLarge
        );
        assert_eq!(unfitted, body);
    }

    #[test]
    fn messages_tool_image_uses_nested_budget_and_redacts_base64() {
        let wire = ImageWire::Messages;
        let body = json!({"messages": [{
            "role": "user",
            "content": [
                {"type":"tool_result","tool_use_id":"unrelated","content":"plain","is_error":false},
                {"type":"tool_result","tool_use_id":"image-tool","content":"image metadata","is_error":false},
            ],
        }]});
        let image = ModelImage {
            mime: ImageMime::Jpeg,
            output: OutputReference {
                id: "fixture".into(),
                offset: 0,
                bytes: 4,
            },
        };
        let target = ToolImageTarget::MessageBlock {
            message: 0,
            block: 1,
        };
        let mut expected = body.clone();
        expected["messages"][0]["content"][1]["content"] = json!([
            {"type":"text","text":"image metadata"},
            {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"AAAAAA=="}},
        ]);
        let exact = json_bytes(&expected);
        let mut fitted = body.clone();
        attach_tool_images(
            wire,
            &mut fitted,
            vec![(target, image.clone())],
            exact,
            &mut |_| Ok(vec![0; 4]),
            None,
        );
        assert_eq!(fitted, expected);
        assert!(has_inline_images(wire, &fitted));
        redact_images(wire, &mut fitted);
        assert!(!fitted.to_string().contains("AAAAAA=="));
        assert_eq!(
            fitted["messages"][0]["content"][1]["tool_use_id"],
            "image-tool"
        );
        let mut unfitted = body.clone();
        attach_tool_images(
            wire,
            &mut unfitted,
            vec![(target, image)],
            exact - 1,
            &mut |_| panic!("over-budget tool image must not load"),
            None,
        );
        assert_eq!(unfitted, body);
    }
}
