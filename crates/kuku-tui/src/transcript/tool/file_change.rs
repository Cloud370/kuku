use kuku_client::{FileCommitState, FileMutationFacts};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::markup::{
    file_source, number_gutter, number_line, render_literal, wrap_bounded, wrap_with_indent,
};
use crate::text::{cap, shorten, shorten_middle, single_line};
use crate::theme::{TranscriptTone, ERROR, MUTED, SUCCESS, WARNING};
use crate::transcript::MAX_VISUAL_ROWS;

use super::{
    tool_operation_color, Row, Tool, ToolDetails, ToolView, MAX_CHANGE_ROWS, MAX_INSPECTION_ROWS,
    MAX_PREVIEW_BYTES,
};

mod diff;
use diff::Presentation;

#[derive(Debug, Clone, Default)]
pub(crate) enum OutputFormat {
    #[default]
    Text,
    Json,
    Change(FileChange),
}

impl ToolView {
    pub(crate) fn formatted_lines(&self, width: usize) -> Option<Vec<Line<'static>>> {
        match &self.format {
            OutputFormat::Text => None,
            OutputFormat::Json => Some(file_source(&self.details, "json")),
            OutputFormat::Change(change) => Some(change.lines(width, MAX_VISUAL_ROWS)),
        }
    }
}

impl Tool {
    pub(super) fn render_change(
        &self,
        change: &FileChange,
        width: usize,
        details: ToolDetails,
    ) -> Vec<Row> {
        let path = self.invocation();
        let mut rows = vec![compact_change_row(change, path, width)];
        if change.unconfirmed {
            rows.push(Row::caption("Durability unconfirmed", "", WARNING, width));
        }
        rows.extend(match details {
            ToolDetails::Collapsed | ToolDetails::CompactFile => change.compact_preview(width),
            ToolDetails::Inspection | ToolDetails::ExpandedFile => change.expanded_rows(width),
            _ => change.preview_rows(
                width,
                Presentation::Expanded(MAX_CHANGE_ROWS),
                MAX_CHANGE_ROWS,
            ),
        });
        if details == ToolDetails::Inspection {
            self.render_arguments(&mut rows, width, 0);
        }
        if !matches!(
            details,
            ToolDetails::CompactFile | ToolDetails::ExpandedFile
        ) {
            rows.push(Row::plain(Line::default()));
        }
        rows
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FileChange {
    tool: &'static str,
    preview: String,
    created: bool,
    preview_line_numbers: Vec<u64>,
    first_changed_line: Option<u64>,
    stats: ChangeStats,
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
            "file_write" => Some(Self::from_write(result, content, truncated)),
            "file_edit" => Some(Self::from_diff("edit_file", &result["change"])),
            "file_remove" => {
                let stats = ChangeStats::Text(result["bytes_removed"].as_u64().map_or_else(
                    || "Removed".to_owned(),
                    |bytes| format!("{bytes} {} removed", plural(bytes, "byte", "bytes")),
                ));
                Some(Self::new("remove_file", None, stats, false, false))
            }
            _ => None,
        }?;
        change.unconfirmed = FileMutationFacts::from_structured(result)
            .is_ok_and(|facts| facts.commit_state == FileCommitState::PublishedUnconfirmed);
        Some(change)
    }

    fn from_write(result: &Value, content: Option<&str>, truncated: bool) -> Self {
        let created = result["created"].as_bool().unwrap_or_else(|| {
            result
                .get("content_hash_before")
                .is_some_and(Value::is_null)
        });
        let count = result["line_count"].as_u64();
        if created {
            let stats = count.map_or_else(
                || ChangeStats::Text(String::new()),
                |added| ChangeStats::Diff { added, removed: 0 },
            );
            let mut change = Self::new("write_file", content, stats, true, truncated);
            let source = std::mem::take(&mut change.preview);
            for (index, line) in source.lines().enumerate() {
                if index != 0 {
                    change.preview.push('\n');
                }
                change.preview.push('+');
                change.preview.push_str(line);
                change.preview_line_numbers.push(index as u64 + 1);
            }
            change.created = true;
            return change;
        }
        let change = &result["change"];
        if change["first_changed_line"].is_u64()
            || (change["added_lines"].is_u64() && change["removed_lines"].is_u64())
        {
            return Self::from_diff("write_file", change);
        }
        let stats = count.map_or_else(
            || ChangeStats::Text(String::new()),
            ChangeStats::WrittenLines,
        );
        Self::new("write_file", content, stats, false, truncated)
    }

    fn from_diff(tool: &'static str, change: &Value) -> Self {
        let stats = match (
            change["added_lines"].as_u64(),
            change["removed_lines"].as_u64(),
        ) {
            (Some(added), Some(removed)) => ChangeStats::Diff { added, removed },
            _ => ChangeStats::Text(String::new()),
        };
        let mut diff = Self::new(
            tool,
            change["preview"].as_str(),
            stats,
            true,
            change["preview_truncated"] == true,
        );
        diff.preview_line_numbers = change["preview_line_numbers"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|line| line.as_u64().unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        diff.first_changed_line = change["first_changed_line"]
            .as_u64()
            .filter(|line| *line > 0);
        diff
    }

    pub(super) fn tool(&self) -> &'static str {
        self.tool
    }

    pub(super) fn verb(&self) -> &'static str {
        match self.tool {
            "write_file" if self.created => "Added",
            "write_file" if !self.diff => "Wrote",
            "remove_file" => "Removed",
            _ => "Edited",
        }
    }

    fn color(&self) -> ratatui::style::Color {
        match self.tool {
            "write_file" if self.created => SUCCESS,
            "remove_file" => ERROR,
            _ => tool_operation_color(self.tool),
        }
    }

    pub(super) fn diff(&self) -> Option<&str> {
        self.diff
            .then_some(self.preview.as_str())
            .filter(|diff| !diff.is_empty())
    }

    pub(super) fn expanded_rows(&self, width: usize) -> Vec<Row> {
        self.preview_rows(
            width,
            Presentation::Expanded(MAX_INSPECTION_ROWS),
            MAX_INSPECTION_ROWS,
        )
    }

    pub(super) fn compact_preview(&self, width: usize) -> Vec<Row> {
        self.preview_rows(width, Presentation::Compact, 2)
    }

    fn preview_rows(&self, width: usize, presentation: Presentation, limit: usize) -> Vec<Row> {
        let columns = Row::detail_width(width);
        let diff = self.diff();
        let lines = match (diff, presentation) {
            (Some(text), _) => diff::render(
                text,
                &self.preview_line_numbers,
                columns,
                presentation,
                self.truncated,
            ),
            (None, Presentation::Compact) => self.write_preview(columns, limit),
            (None, Presentation::Expanded(_)) => self.lines(columns, limit),
        };
        let mut rows = lines
            .into_iter()
            .map(|line| Row::plain(line).with_detail_rule(width))
            .collect::<Vec<_>>();
        if diff.is_some() && matches!(presentation, Presentation::Expanded(_)) && !rows.is_empty() {
            rows.insert(0, Row::plain(Line::default()));
            rows.push(Row::plain(Line::default()));
        }
        rows
    }

    fn write_preview(&self, width: usize, limit: usize) -> Vec<Line<'static>> {
        if width == 0 {
            return Vec::new();
        }
        let digits = self.line_number_width(width);
        let room = width - number_gutter(digits);
        let mut end = 0;
        let shown = self
            .preview
            .split_inclusive('\n')
            .filter(|line| {
                end += line.len();
                !line.trim().is_empty()
            })
            .take(limit)
            .count();
        let has_more = self.truncated || !self.preview[end..].trim().is_empty();
        Self::write_source(&self.preview[..end])
            .into_iter()
            .enumerate()
            .filter(|(_, line)| {
                line.spans
                    .iter()
                    .any(|span| !span.content.trim().is_empty())
            })
            .enumerate()
            .map(|(index, (source, line))| {
                let omitted = line.width() > room || (has_more && index + 1 == shown);
                let max_cols = room - usize::from(omitted);
                let mut line = if max_cols == 0 {
                    Line::default()
                } else if line.width() > max_cols {
                    wrap_bounded(vec![line], max_cols, 1)
                        .into_iter()
                        .next()
                        .unwrap_or_default()
                } else {
                    line
                };
                if omitted {
                    line.push_span(Span::styled("…", Style::new().fg(MUTED)));
                }
                number_line(line, source + 1, digits)
            })
            .collect()
    }

    fn write_source(text: &str) -> Vec<Line<'static>> {
        if text.is_empty() {
            return Vec::new();
        }
        let text = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text);
        render_literal(text, Style::new())
    }

    fn line_number_width(&self, width: usize) -> usize {
        let digits = self.preview.lines().count().to_string().len();
        if width >= digits + 7 {
            digits
        } else {
            0
        }
    }

    pub(super) fn stats(&self) -> String {
        self.stats.text()
    }

    fn lines(&self, width: usize, limit: usize) -> Vec<Line<'static>> {
        if self.diff {
            return diff::render(
                &self.preview,
                &self.preview_line_numbers,
                width,
                Presentation::Expanded(limit),
                self.truncated,
            );
        }
        if self.preview.is_empty() {
            return Vec::new();
        }
        let digits = self.line_number_width(width);
        let mut lines = Self::write_source(&self.preview)
            .into_iter()
            .enumerate()
            .map(|(index, line)| number_line(line, index + 1, digits))
            .collect::<Vec<_>>();
        if self.truncated {
            if let Some(last) = lines.last_mut() {
                last.push_span(Span::styled("…", Style::new().fg(MUTED)));
            }
        }
        wrap_with_indent(lines, width, limit, &" ".repeat(number_gutter(digits)))
    }

    fn new(
        tool: &'static str,
        content: Option<&str>,
        stats: ChangeStats,
        diff: bool,
        truncated: bool,
    ) -> Self {
        let content = content.unwrap_or_default();
        let head = cap(content, MAX_PREVIEW_BYTES);
        Self {
            tool,
            preview: head.to_owned(),
            created: false,
            preview_line_numbers: Vec::new(),
            first_changed_line: None,
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

#[derive(Debug, Clone)]
enum ChangeStats {
    Diff { added: u64, removed: u64 },
    WrittenLines(u64),
    Text(String),
}

impl ChangeStats {
    fn text(&self) -> String {
        match self {
            Self::Diff { added, removed } => format!("+{added} -{removed}"),
            Self::WrittenLines(count) => format!("{count} {}", plural(*count, "line", "lines")),
            Self::Text(text) => text.clone(),
        }
    }

    fn spans(&self, text: String) -> Vec<Span<'static>> {
        let muted = Style::new().fg(MUTED);
        let mut spans = vec![Span::styled(" (", muted)];
        spans.extend(match self {
            Self::Diff { added, removed } => vec![
                Span::styled(format!("+{added}"), Style::new().fg(SUCCESS)),
                Span::styled(" ", muted),
                Span::styled(format!("-{removed}"), Style::new().fg(ERROR)),
            ],
            Self::WrittenLines(count) => {
                Row::metric(
                    "",
                    count.to_string(),
                    plural(*count, " line", " lines"),
                    TranscriptTone::Value,
                )
                .spans
            }
            Self::Text(_) => vec![Span::styled(text, muted)],
        });
        spans.push(Span::styled(")", muted));
        spans
    }
}

pub(super) fn compact_change_row(change: &FileChange, path: &str, width: usize) -> Row {
    let verb = shorten(&format!("{:<9}", change.verb()), width);
    let room = width.saturating_sub(verb.width());
    let stats = change.stats.text();
    let stats_width = stats.width() + 3;
    let show_stats = !stats.is_empty() && stats_width < room;
    let room = room - usize::from(show_stats) * stats_width;
    let position = change
        .first_changed_line
        .filter(|_| change.preview.is_empty())
        .map(|line| format!(" · line {line}"))
        .filter(|position| room > position.width() + 1)
        .unwrap_or_default();
    let path = shorten_middle(&single_line(path), room - position.width());
    let mut spans = vec![
        Span::styled(verb, Style::new().fg(change.color())),
        Span::styled(path, Style::new().bold()),
    ];
    if show_stats {
        spans.extend(change.stats.spans(stats));
    }
    if !position.is_empty() {
        spans.push(Span::styled(position, Style::new().fg(MUTED)));
    }
    Row::plain(Line::from(spans))
}
