use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::markup::{file_source, render_diff, render_markdown, wrap_bounded};
use crate::text::{cap, single_line};
use crate::theme::{ACCENT, MUTED};
use std::path::Path;

use super::{
    join_details, Row, Tool, ToolDetails, MAX_CHANGE_ROWS, MAX_INSPECTION_ROWS, MAX_PREVIEW_BYTES,
};

impl Tool {
    pub(super) fn render_change(
        &self,
        change: &FileChange,
        width: usize,
        inset: u16,
        details: ToolDetails,
    ) -> Vec<Row> {
        let frame = crate::theme::CardFrame::new(width);
        let path = self.invocation();
        let title = join_details([
            self.title().as_ref(),
            self.state.label().0,
            single_line(path).as_str(),
        ]);
        let mut rows = vec![Row::plain(frame.title(&title, ACCENT))];
        let mut clipped = false;
        if details != ToolDetails::Collapsed {
            let limit = if details == ToolDetails::Inspection {
                MAX_INSPECTION_ROWS
            } else {
                MAX_CHANGE_ROWS
            };
            let lines = change.lines(path, frame.content_width(), limit);
            clipped = lines.len() > limit;
            rows.extend(
                lines
                    .into_iter()
                    .take(limit)
                    .map(|line| Row::plain(frame.line(line))),
            );
        }
        let footer = if clipped {
            format!("{} · …", change.stats)
        } else {
            change.stats.clone()
        };
        Row::pad_framed_body(&mut rows, &frame);
        rows.push(Row::plain(frame.foot(&footer)));
        if details == ToolDetails::Inspection {
            self.render_arguments(&mut rows, width, inset);
        }
        rows.push(Row::plain(Line::default()));
        rows
    }
}

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

    pub(super) fn lines(&self, path: &str, width: usize, limit: usize) -> Vec<Line<'static>> {
        let text = self
            .preview
            .strip_suffix("\r\n")
            .or_else(|| self.preview.strip_suffix('\n'))
            .unwrap_or(&self.preview);
        let language = Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        let mut lines = if text.is_empty() {
            Vec::new()
        } else if self.diff {
            render_diff(text)
        } else if language.eq_ignore_ascii_case("md") || language.eq_ignore_ascii_case("markdown") {
            let mut lines = render_markdown(text, width, limit);
            if lines.last().is_some_and(|line| line.spans.is_empty()) {
                lines.pop();
            }
            lines
        } else {
            file_source(text, language)
        };
        if self.truncated {
            if let Some(last) = lines.last_mut() {
                last.push_span(Span::styled("…", Style::new().fg(MUTED)));
            }
        }
        wrap_bounded(lines, width, limit)
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
