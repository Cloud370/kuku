use std::collections::HashSet;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};

use crate::execution::{ModelImage, MAX_MODEL_IMAGE_BYTES};

const MAX_IMAGES_PER_REQUEST: usize = 4;

pub(crate) fn attach_tool_images(
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
    for (index, image) in images.into_iter().rev() {
        if attached.len() == MAX_IMAGES_PER_REQUEST {
            break;
        }
        if attached.contains(&image.output.id) || image.output.bytes > MAX_MODEL_IMAGE_BYTES as u64
        {
            continue;
        }
        let output = &mut body["input"][index]["output"];
        let mut parts = json!([
            {"type": "input_text", "text": output},
            {"type": "input_image", "image_url": ""},
        ]);
        let mut url = format!("data:{};base64,", image.mime.as_str());
        let additional = json_bytes(&parts) - json_bytes(output)
            + url.len()
            + (image.output.bytes as usize).div_ceil(3) * 4;
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
        parts[1]["image_url"] = Value::String(url);
        *output = parts;
        attached.insert(image.output.id);
        available -= additional;
    }
}

pub(crate) fn redact_tool_images(body: &mut Value) {
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        if item["type"] != "function_call_output" {
            continue;
        }
        if let Some(parts) = item["output"].as_array_mut() {
            for part in parts
                .iter_mut()
                .filter(|part| part["type"] == "input_image")
            {
                if let Some(url) = part["image_url"]
                    .as_str()
                    .filter(|url| url.starts_with("data:"))
                {
                    part["image_url"] = json!(format!("<omitted image data, {} bytes>", url.len()));
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
    use serde_json::json;

    use super::{attach_tool_images, json_bytes, redact_tool_images};
    use crate::execution::{ImageMime, ModelImage, OutputReference};

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
        attach_tool_images(&mut preview, candidates.clone(), usize::MAX, &mut |_| {
            Ok(vec![0; 4])
        });
        let exact = json_bytes(&preview);
        assert_eq!(preview["input"][0]["output"], "first");
        assert_eq!(
            preview["input"][1]["output"][1]["image_url"],
            "data:image/png;base64,AAAAAA=="
        );

        let mut limited = body.clone();
        attach_tool_images(&mut limited, candidates.clone(), exact, &mut |_| {
            Ok(vec![0; 4])
        });
        assert_eq!(limited, preview);
        redact_tool_images(&mut limited);
        assert!(!limited.to_string().contains("data:image/png"));
        assert_eq!(limited["input"][1]["output"][1]["type"], "input_image");

        let mut recent_only = body.clone();
        attach_tool_images(
            &mut recent_only,
            candidates.into_iter().skip(1).collect(),
            exact - 1,
            &mut |_| panic!("image above the byte limit must never load"),
        );
        assert_eq!(recent_only, body);
    }
}
