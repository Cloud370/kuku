use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::OutputReference;

/// Maximum size of one encoded PNG or JPEG.
pub const MAX_MODEL_IMAGE_BYTES: usize = 1024 * 1024;
/// Maximum number of user images in one input.
pub const MAX_INPUT_IMAGES: usize = 4;

/// Identity of a user image saved in the recipient's Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputImage {
    pub(crate) mime: ImageMime,
    pub(crate) digest: String,
    pub(crate) bytes: usize,
}

impl InputImage {
    /// Identify a bounded, encoded PNG or JPEG before saving it with the Session.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let mime = ImageMime::detect(bytes)
            .filter(|_| bytes.len() <= MAX_MODEL_IMAGE_BYTES)
            .ok_or_else(|| {
                crate::Error::InvalidArgument("image must be PNG or JPEG and at most 1 MiB".into())
            })?;
        Ok(Self {
            mime,
            digest: format!("{:x}", Sha256::digest(bytes)),
            bytes: bytes.len(),
        })
    }

    /// Media type of the encoded image.
    pub fn media_type(&self) -> &'static str {
        self.mime.as_str()
    }

    /// Number of encoded bytes before base64 wrapping.
    pub fn byte_len(&self) -> usize {
        self.bytes
    }

    pub(crate) fn key(&self, agent: &str) -> String {
        format!("input:{agent}:{}", self.digest)
    }
}

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
