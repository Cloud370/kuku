use ratatui::style::Color;
use ratatui::text::Line;
use serde_json::Value;

use crate::text::{cap, single_line};
use crate::theme::{CardFrame, SURFACE_TEXT};

use super::{capped, Outcome, Row, Tool, MAX_DETAIL_BYTES};

#[derive(Debug, Clone)]
pub(super) struct CommandPreview {
    complete: bool,
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

impl Outcome {
    pub(super) fn concise_command(&self) -> bool {
        self.command_preview
            .as_ref()
            .is_some_and(|preview| preview.complete && preview.notes.is_empty())
            && self.status == "ok"
            && self
                .exit_code
                .is_some_and(|code| code != 0 || !self.attention)
    }
}

impl Tool {
    pub(super) fn command_heading(&self, frame: &CardFrame) -> Row {
        let title = if self.start_summary.is_empty() {
            "Run command"
        } else {
            &self.start_summary
        };
        Row::plain(frame.title(&single_line(title), SURFACE_TEXT))
    }

    pub(super) fn shell_command(&self) -> &str {
        self.state
            .outcome()
            .filter(|outcome| !outcome.invocation.is_empty())
            .map_or(self.invocation.as_str(), |outcome| {
                outcome.invocation.as_str()
            })
    }
}

pub(super) fn finish(
    frame: CardFrame,
    mut rows: Vec<Row>,
    (status, color): (String, Color),
) -> Vec<Row> {
    for row in rows.iter_mut().skip(1) {
        row.line = frame.line(std::mem::take(&mut row.line));
        row.inset = frame.content_inset() as u16;
    }
    rows.push(Row::plain(frame.status_foot(&status, color)));
    rows.push(Row::plain(Line::default()));
    rows
}
