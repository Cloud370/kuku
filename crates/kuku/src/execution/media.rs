use serde::{Deserialize, Serialize};

use super::OutputReference;

pub(crate) const MAX_MODEL_IMAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ImageMime {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
}

impl ImageMime {
    pub(crate) fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Some(Self::Jpeg)
        } else {
            None
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelImage {
    pub(crate) mime: ImageMime,
    pub(crate) output: OutputReference,
}

impl ModelImage {
    pub(crate) fn from_tool_result(value: &serde_json::Value) -> Option<Self> {
        if value.get("kind")?.as_str()? != "image_view" {
            return None;
        }
        serde_json::from_value(value.get("image")?.clone()).ok()
    }

    pub(crate) fn model_facts(&self) -> serde_json::Value {
        serde_json::json!({"kind": "image_view", "image": self})
    }
}
