use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::markup::{render_diff, render_literal};
use crate::text::cap;
use crate::theme::{MUTED, SUCCESS};

use super::MAX_PREVIEW_BYTES;

#[derive(Debug, Clone)]
pub(super) struct FileChange {
    tool: &'static str,
    preview: String,
    pub(super) stats: String,
    diff: bool,
    truncated: bool,
}

impl FileChange {
    pub(super) fn from_result(result: &Value, truncated: bool) -> Option<Self> {
        let content = result["raw_text_after"].as_str();
        match result["kind"].as_str()? {
            "file_write" => {
                let stats = result["line_count"].as_u64().map_or_else(
                    || "Written".to_owned(),
                    |count| format!("{count} {} written", plural(count, "line", "lines")),
                );
                Some(Self::new("write_file", content, stats, false, truncated))
            }
            "file_edit" => {
                let change = &result["change"];
                let added = change["added_lines"].as_u64()?;
                let removed = change["removed_lines"].as_u64()?;
                let stats = if added == 0 && removed == 0 {
                    "No lines changed".to_owned()
                } else {
                    format!("+{added} −{removed}")
                };
                Some(Self::new(
                    "edit_file",
                    change["preview"].as_str(),
                    stats,
                    true,
                    change["preview_truncated"].as_bool().unwrap_or(false),
                ))
            }
            _ => None,
        }
    }

    pub(super) fn is_for(&self, tool: &str) -> bool {
        self.tool == tool
    }

    pub(super) fn diff(&self) -> Option<&str> {
        self.diff
            .then_some(self.preview.as_str())
            .filter(|diff| !diff.is_empty())
    }

    pub(super) fn lines(&self) -> Vec<Line<'static>> {
        let text = self.preview.trim_end_matches('\n');
        let mut lines = if text.is_empty() {
            Vec::new()
        } else if self.diff {
            render_diff(text)
        } else {
            render_literal(text, Style::new())
        };
        if self.truncated {
            if let Some(last) = lines.last_mut() {
                last.push_span(Span::styled("…", Style::new().fg(MUTED)));
            }
        }
        lines
    }

    pub(super) fn footer(&self) -> Line<'static> {
        Line::styled(self.stats.clone(), Style::new().fg(SUCCESS))
    }

    fn new(
        tool: &'static str,
        content: Option<&str>,
        stats: String,
        diff: bool,
        truncated: bool,
    ) -> Self {
        let content = content.unwrap_or_default();
        let head = cap(content, MAX_PREVIEW_BYTES);
        Self {
            tool,
            preview: head.to_owned(),
            stats,
            diff,
            truncated: truncated || head.len() != content.len(),
        }
    }
}

fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}
