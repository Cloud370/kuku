use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) marker: Option<ImageMarker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transparency: Option<bool>,
}

/// Exact byte position and text of an image marker in its input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMarker {
    /// Byte offset in the original UTF-8 input.
    pub offset: usize,
    /// Nonempty, single-line display label.
    pub label: String,
}

impl ImageMarker {
    /// Return the marker's range if it matches the supplied input.
    pub fn range_in(&self, text: &str) -> Option<std::ops::Range<usize>> {
        let range = self.offset..self.offset.checked_add(self.label.len())?;
        (!self.label.is_empty()
            && !self.label.contains(['\n', '\r'])
            && text.get(range.clone()) == Some(&self.label))
        .then_some(range)
    }
}

impl InputImage {
    /// Identify a bounded, encoded PNG or JPEG before saving it with the Session.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let mime = ImageMime::detect(bytes)
            .filter(|_| bytes.len() <= MAX_MODEL_IMAGE_BYTES)
            .ok_or_else(|| {
                crate::Error::InvalidArgument("image must be PNG or JPEG and at most 1 MiB".into())
            })?;
        let transparency = validate_image(bytes, mime)?;
        Ok(Self {
            mime,
            digest: Self::digest(bytes),
            bytes: bytes.len(),
            marker: None,
            transparency: Some(transparency),
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

    /// Associate the stored image with a marker in its input.
    pub fn with_marker(mut self, marker: ImageMarker) -> Self {
        self.marker = Some(marker);
        self
    }

    /// Return the optional marker carried by this input image.
    pub fn marker(&self) -> Option<&ImageMarker> {
        self.marker.as_ref()
    }

    /// Whether the validated image contains visibly transparent pixels.
    pub fn is_transparent(&self) -> bool {
        self.transparency.unwrap_or(false)
    }

    pub(crate) fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    pub(crate) fn key(&self, agent: &str) -> String {
        format!("input:{agent}:{}", self.digest)
    }
}

pub(crate) fn validate_image(bytes: &[u8], mime: ImageMime) -> crate::Result<bool> {
    let invalid = |error: image::ImageError| {
        crate::Error::InvalidArgument(format!("cannot decode image: {error}"))
    };
    let format = match mime {
        ImageMime::Png => ImageFormat::Png,
        ImageMime::Jpeg => ImageFormat::Jpeg,
    };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(invalid)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > 32 * 1024 * 1024 {
        return Err(crate::Error::InvalidArgument(
            "image exceeds the pixel limit".into(),
        ));
    }
    let decoded = DynamicImage::from_decoder(decoder).map_err(invalid)?;
    Ok(match decoded {
        DynamicImage::ImageLumaA8(image) => image.pixels().any(|pixel| pixel[1] != u8::MAX),
        DynamicImage::ImageLumaA16(image) => image.pixels().any(|pixel| pixel[1] != u16::MAX),
        DynamicImage::ImageRgba8(image) => image.pixels().any(|pixel| pixel[3] != u8::MAX),
        DynamicImage::ImageRgba16(image) => image.pixels().any(|pixel| pixel[3] != u16::MAX),
        _ => false,
    })
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
