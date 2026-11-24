use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};

use crate::execution::{ImageMime, MAX_MODEL_IMAGE_BYTES};
use crate::tool::{ToolDefinition, ToolResultEnvelope};

use super::common::{
    FileCancellation, IoLimit, LimitedReader, StableFile, file_read_error, required_path,
    resolve_path,
};

pub(crate) fn view_image_definition() -> ToolDefinition {
    ToolDefinition {
        name: "view_image".into(),
        description: "View a workspace PNG or JPEG, such as a screenshot or diagram.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "Image path in the workspace."}},
            "required": ["path"],
            "additionalProperties": false,
        }),
        read_only: true,
        max_result_chars: 1024,
        risk: "read".into(),
    }
}

pub(crate) fn read_image(
    args: &Value,
    workspace: &Path,
    cancelled: &FileCancellation<'_>,
) -> Result<(ImageMime, Vec<u8>), ToolResultEnvelope> {
    let path = required_path("view_image", args)?;
    let resolved = resolve_path(workspace, path)?;
    if workspace.join(path) != resolved.path {
        return Err(ToolResultEnvelope::blocked(
            "image target changed after authorization",
            "request the image at its new path",
        ));
    }
    let mut file = StableFile::open(&resolved.path, cancelled)
        .map_err(|error| file_read_error(&resolved.path, error))?;
    let limit = IoLimit("image exceeds 1 MiB, reduce its size before viewing it");
    let mut bytes_left = MAX_MODEL_IMAGE_BYTES as u64;
    file.check_size(bytes_left, limit)
        .map_err(|error| file_read_error(&resolved.path, error))?;
    let mut bytes = Vec::new();
    LimitedReader::new(&mut file, &mut bytes_left, limit)
        .read_to_end(&mut bytes)
        .and_then(|_| file.verify(&resolved.path))
        .map_err(|error| file_read_error(&resolved.path, error))?;
    let mime = ImageMime::detect(&bytes).ok_or_else(|| {
        ToolResultEnvelope::error("unsupported image", "view_image accepts PNG and JPEG files")
    })?;
    Ok((mime, bytes))
}
