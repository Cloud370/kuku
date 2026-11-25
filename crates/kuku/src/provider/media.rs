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
    pub(crate) tools: Vec<(usize, ModelImage)>,
    pub(crate) user: Vec<(usize, InputImage)>,
    pub(crate) current_user_index: Option<usize>,
    pub(crate) current_input_offset: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) enum ImageWire {
    ChatCompletions,
    Responses,
}

impl ImageWire {
    pub(super) fn for_kind(kind: ProviderKind) -> Option<Self> {
        match kind {
            ProviderKind::OpenAiCompatible => Some(Self::ChatCompletions),
            ProviderKind::OpenAiResponses => Some(Self::Responses),
            ProviderKind::Anthropic => None,
        }
    }

    fn items(self) -> &'static str {
        match self {
            Self::ChatCompletions => "messages",
            Self::Responses => "input",
        }
    }

    fn image_part(self, url: &str) -> Value {
        match self {
            Self::ChatCompletions => json!({"type": "image_url", "image_url": {"url": url}}),
            Self::Responses => json!({"type": "input_image", "image_url": url}),
        }
    }

    fn image_url(self, part: &mut Value) -> Option<&mut Value> {
        match part["type"].as_str() {
            Some("image_url") if matches!(self, Self::ChatCompletions) => {
                part.get_mut("image_url")?.get_mut("url")
            }
            Some("input_image") if matches!(self, Self::Responses) => part.get_mut("image_url"),
            _ => None,
        }
    }
}

pub(super) fn attach_tool_images(
    wire: ImageWire,
    body: &mut Value,
    images: Vec<(usize, ModelImage)>,
    limit: usize,
    load: &mut dyn FnMut(&ModelImage) -> crate::Result<Vec<u8>>,
) {
    if images.is_empty() {
        return;
    }
    let mut available = limit.saturating_sub(json_bytes(body));
    let mut attached = HashSet::new();
    let mut chat_insertions = Vec::new();
    for (index, image) in images.into_iter().rev() {
        if attached.len() == MAX_IMAGES_PER_REQUEST {
            break;
        }
        if attached.contains(&image.output.id) || image.output.bytes > MAX_MODEL_IMAGE_BYTES as u64
        {
            continue;
        }
        let mut url = format!("data:{};base64,", image.mime.as_str());
        let item = &mut body[wire.items()][index];
        let part = wire.image_part(&url);
        let (mut value, additional) = match wire {
            ImageWire::Responses => {
                let parts = json!([{"type": "input_text", "text": item["output"]}, part]);
                let delta = json_bytes(&parts) - json_bytes(&item["output"]);
                (parts, delta)
            }
            ImageWire::ChatCompletions => {
                let content = format!(
                    "Image from view_image tool call {} (tool output, not a new user request).",
                    item["tool_call_id"]
                );
                let message = json!({
                    "role": "user",
                    "content": [{"type": "text", "text": content}, part],
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
        STANDARD.encode_string(bytes, &mut url);
        match wire {
            ImageWire::Responses => {
                *wire.image_url(&mut value[1]).expect("Responses image part") = Value::String(url);
                item["output"] = value;
            }
            ImageWire::ChatCompletions => {
                *wire
                    .image_url(&mut value["content"][1])
                    .expect("Chat image part") = Value::String(url);
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
            ImageWire::Responses => content.as_array().expect("Responses content array").clone(),
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
            let url = format!("data:{};base64,", image.media_type());
            parts.push(wire.image_part(&url));
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
                let part = wire
                    .image_url(&mut new_content[first_image + index])
                    .expect("user image part");
                let mut url = part.as_str().expect("image URL string").to_owned();
                STANDARD.encode_string(bytes, &mut url);
                *part = Value::String(url);
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

pub(super) fn redact_images(wire: ImageWire, body: &mut Value) {
    let Some(input) = body.get_mut(wire.items()).and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        let field = match (wire, item["type"].as_str()) {
            (ImageWire::Responses, Some("function_call_output")) => "output",
            (ImageWire::Responses, Some("message")) | (ImageWire::ChatCompletions, _) => "content",
            _ => continue,
        };
        if let Some(parts) = item[field].as_array_mut() {
            for part in parts {
                if let Some(url) = wire.image_url(part)
                    && let Some(size) = url
                        .as_str()
                        .filter(|url| url.starts_with("data:"))
                        .map(str::len)
                {
                    *url = json!(format!("<omitted image data, {size} bytes>"));
                }
            }
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

    use super::{ImageWire, attach_tool_images, attach_user_images, json_bytes, redact_images};
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
        let candidates = vec![(0, image.clone()), (1, image)];
        let mut preview = body.clone();
        attach_tool_images(
            ImageWire::Responses,
            &mut preview,
            candidates.clone(),
            usize::MAX,
            &mut |_| Ok(vec![0; 4]),
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
        );
        assert_eq!(limited, preview);
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
}
