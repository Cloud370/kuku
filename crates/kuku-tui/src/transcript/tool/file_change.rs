use std::path::Path;

use kuku_client::{FileCommitState, FileMutationFacts};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::markup::{file_source, render_diff, render_markdown, wrap_with_indent};
use crate::text::{cap, shorten, shorten_middle, single_line};
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::{
    body_inset, join_details, render_rows, Row, Tool, ToolDetails, MAX_CHANGE_ROWS,
    MAX_INSPECTION_ROWS, MAX_PREVIEW_BYTES,
};

impl Tool {
    pub(super) fn render_change(
        &self,
        change: &FileChange,
        width: usize,
        details: ToolDetails,
    ) -> Vec<Row> {
        if matches!(
            details,
            ToolDetails::CompactFile | ToolDetails::ExpandedFile
        ) {
            let open = details == ToolDetails::ExpandedFile;
            let path = self.invocation();
            let mut rows = vec![compact_change_row(change, path, width)];
            if open {
                let inset = body_inset(width);
                let preview = change.lines(path, width - usize::from(inset), MAX_INSPECTION_ROWS);
                rows.extend(render_rows(preview, width, MAX_INSPECTION_ROWS + 1, inset));
            }
            return rows;
        }
        let frame = crate::theme::CardFrame::new(width);
        let path = self.invocation();
        let status = if change.unconfirmed {
            "applied · durability unconfirmed"
        } else {
            self.state.label().0
        };
        let title = join_details([self.title().as_ref(), status, single_line(path).as_str()]);
        let color = if change.unconfirmed { WARNING } else { ACCENT };
        let mut rows = vec![Row::plain(frame.title(&title, color))];
        let body_width = frame.content_width();
        let mut clipped = false;
        if details != ToolDetails::Collapsed {
            let limit = if details == ToolDetails::Inspection {
                MAX_INSPECTION_ROWS
            } else {
                MAX_CHANGE_ROWS
            };
            let lines = change.lines(path, body_width, limit);
            clipped = lines.len() > limit;
            rows.extend(
                lines
                    .into_iter()
                    .take(limit)
                    .map(|line| Row::plain(frame.line(line))),
            );
        }
        if details == ToolDetails::Inspection {
            let mut arguments = Vec::new();
            self.render_arguments(&mut arguments, body_width, 0);
            rows.extend(arguments.into_iter().map(|row| row.framed(&frame)));
        }
        let footer = if clipped {
            format!("{} · …", change.stats)
        } else {
            change.stats.clone()
        };
        Row::pad_framed_body(&mut rows, &frame);
        rows.push(Row::plain(frame.foot(&footer)));
        rows.push(Row::plain(Line::default()));
        rows
    }
}

#[derive(Debug, Clone)]
pub(super) struct FileChange {
    tool: &'static str,
    preview: String,
    preview_line_numbers: Vec<u64>,
    pub(super) stats: String,
    pub(super) unconfirmed: bool,
    diff: bool,
    truncated: bool,
}

impl FileChange {
    pub(super) fn from_result(result: &Value, truncated: bool) -> Option<Self> {
        let kind = result["kind"].as_str()?;
        let content = result["raw_text_after"].as_str();
        let truncated = truncated || result["raw_text_after_truncated"] == true;
        let mut change = match kind {
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
                let mut edit = Self::new(
                    "edit_file",
                    change["preview"].as_str(),
                    stats,
                    true,
                    change["preview_truncated"].as_bool().unwrap_or(false),
                );
                edit.preview_line_numbers = change["preview_line_numbers"]
                    .as_array()
                    .and_then(|values| values.iter().map(Value::as_u64).collect())
                    .unwrap_or_default();
                Some(edit)
            }
            "file_remove" => {
                let stats = result["bytes_removed"].as_u64().map_or_else(
                    || "Removed".to_owned(),
                    |bytes| format!("{bytes} {} removed", plural(bytes, "byte", "bytes")),
                );
                Some(Self::new("remove_file", None, stats, false, false))
            }
            _ => None,
        }?;
        change.unconfirmed = FileMutationFacts::from_structured(result)
            .is_ok_and(|facts| facts.commit_state == FileCommitState::PublishedUnconfirmed);
        Some(change)
    }

    pub(super) fn tool(&self) -> &'static str {
        self.tool
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
        let mut gutter = 0;
        let mut lines = if text.is_empty() {
            Vec::new()
        } else if self.diff {
            let (lines, gutter_width) = numbered_diff(text, &self.preview_line_numbers, width);
            gutter = gutter_width;
            lines
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
        wrap_with_indent(lines, width, limit, &" ".repeat(gutter))
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
            preview_line_numbers: Vec::new(),
            stats,
            unconfirmed: false,
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

fn numbered_diff(text: &str, numbers: &[u64], width: usize) -> (Vec<Line<'static>>, usize) {
    let mut lines = render_diff(text);
    let digits = numbers.iter().copied().max().unwrap_or(0).to_string().len();
    let gutter = digits + 4;
    if numbers.is_empty() || width < gutter + 2 {
        return (lines, 0);
    }
    for (line, number) in lines.iter_mut().zip(numbers) {
        if let Some(span) = line.spans.first_mut() {
            if let Some(marker @ ('-' | '+')) = span.content.chars().next() {
                span.content = span.content[1..].to_owned().into();
                line.spans.insert(
                    0,
                    Span::styled(
                        format!("{marker}{number:<digits$} │ "),
                        Style::new().fg(MUTED),
                    ),
                );
            }
        }
    }
    (lines, gutter)
}

fn compact_change_row(change: &FileChange, path: &str, width: usize) -> Row {
    let (action, color) = match change.tool {
        "write_file" => ("Wrote", SUCCESS),
        "remove_file" => ("Removed", WARNING),
        _ => ("Edited", SUCCESS),
    };
    let action = if width >= 36 {
        format!("{action:<8} ")
    } else {
        format!("{action} ")
    };
    let stats = format!("  {}", change.stats);
    let stats = shorten(
        &stats,
        width.saturating_sub(action.width() + 2).min(width / 2),
    );
    let path = shorten_middle(
        &single_line(path),
        width.saturating_sub(action.width() + stats.width()),
    );
    let padding = if width >= 60 {
        22usize
            .saturating_sub(path.width())
            .min(width.saturating_sub(action.width() + path.width() + stats.width()))
    } else {
        0
    };
    let line = Line::from(vec![
        Span::styled(shorten(&action, width), Style::new().fg(color)),
        Span::raw(path),
        Span::raw(" ".repeat(padding)),
        Span::styled(stats, Style::new().fg(MUTED)),
    ]);
    Row::plain(line)
}
