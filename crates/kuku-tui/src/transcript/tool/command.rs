use serde_json::Value;

use crate::text::cap;

use super::{capped, MAX_DETAIL_BYTES};

#[derive(Debug, Clone)]
pub(super) struct CommandPreview {
    pub(super) complete: bool,
    body_end: usize,
    pub(super) notes: String,
}

impl CommandPreview {
    pub(super) fn from_result(
        content: Option<&str>,
        structured: &Value,
        output_id: Option<&str>,
        truncated: bool,
    ) -> Option<Self> {
        if structured["kind"].as_str() != Some("command_result") {
            return None;
        }
        let bytes = structured["output_ref"]["bytes"].as_u64()?;
        let id = output_id?;
        let reference =
            format!("\n\nOutput: {id} ({bytes} captured bytes; read by merged byte offset).");
        let (body, notes) = content?.rsplit_once(&reference)?;
        if notes.contains(['\n', '\r']) || (!notes.is_empty() && !notes.starts_with(' ')) {
            return None;
        }
        Some(Self {
            complete: !truncated
                && structured["capture_complete"] != false
                && structured["preview_truncated"] != true,
            body_end: cap(body.trim_end_matches(['\r', '\n']), MAX_DETAIL_BYTES).len(),
            notes: capped(Some(notes.trim_start()), MAX_DETAIL_BYTES),
        })
    }

    pub(super) fn body<'a>(&self, result: &'a str, concise: bool) -> &'a str {
        let text = &result[..self.body_end];
        if concise && !text.contains("\nstderr:\n") && !text.contains("\nstdout:\n") {
            text.strip_prefix("stdout:\n")
                .or_else(|| text.strip_prefix("stderr:\n"))
                .unwrap_or(text)
        } else {
            text
        }
    }
}
