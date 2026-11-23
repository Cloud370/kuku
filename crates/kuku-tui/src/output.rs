use kuku_client::{OutputChunk, OutputStream};
use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::keys::Shortcut;
use crate::markup::{render_diff, render_literal, wrap, wrap_bounded, wrap_numbered};
use crate::theme::{ACCENT, ERROR, MUTED, WARNING};
use crate::transcript::ToolView;

pub(crate) const OUTPUT_PAGE_BYTES: usize = 4 * 1024;
const MAX_PREVIOUS_OFFSETS: usize = 128;
const RETAINED_INVOCATION_ROWS: usize = 2;
const RETAINED_EVIDENCE_ROWS: usize = 8;

#[derive(Debug)]
pub(crate) struct OutputViewer {
    pub(crate) id: u64,
    pub(crate) tool: ToolView,
    pub(crate) loading: bool,
    pub(crate) problem: Option<String>,
    chunk: Option<OutputChunk>,
    top: usize,
    height: usize,
    row_count: usize,
    offsets: Vec<(u64, usize)>,
    first_line: usize,
    previous_request: bool,
}

impl OutputViewer {
    pub(crate) fn new(id: u64, tool: ToolView) -> Self {
        let loading = tool.output_id.is_some();
        Self {
            id,
            tool,
            loading,
            problem: None,
            chunk: None,
            top: 0,
            height: 1,
            row_count: 0,
            offsets: Vec::new(),
            first_line: 1,
            previous_request: false,
        }
    }

    pub(crate) fn apply(&mut self, chunk: OutputChunk) {
        self.loading = false;
        self.problem = None;
        if let Some(previous) = &self.chunk {
            if self.previous_request {
                if let Some((_, line)) = self.offsets.pop() {
                    self.first_line = line;
                }
            } else if chunk.offset > previous.offset {
                if self.offsets.len() == MAX_PREVIOUS_OFFSETS {
                    self.offsets.remove(0);
                }
                self.offsets.push((previous.offset, self.first_line));
                self.first_line += previous
                    .segments
                    .iter()
                    .flat_map(|segment| segment.as_bytes())
                    .filter(|&&byte| byte == b'\n')
                    .count();
            }
        }
        self.top = 0;
        self.chunk = Some(chunk);
    }

    pub(crate) fn scroll(&mut self, amount: isize) -> Option<u64> {
        let bottom = self.row_count.saturating_sub(self.height);
        let previous = amount.is_negative() && self.top == 0;
        let next = amount > 0 && self.top == bottom;
        self.top = self.top.saturating_add_signed(amount).min(bottom);
        if previous || next {
            return self.request_page(previous);
        }
        None
    }

    pub(crate) fn page_offset(&self, previous: bool) -> Option<u64> {
        let chunk = self.chunk.as_ref()?;
        if previous {
            self.offsets.last().map(|&(offset, _)| offset)
        } else if (chunk.has_more || !chunk.complete)
            && self
                .tool
                .output_range
                .as_ref()
                .is_none_or(|range| chunk.next_offset < range.end)
        {
            Some(chunk.next_offset)
        } else {
            None
        }
    }

    pub(crate) fn request_page(&mut self, previous: bool) -> Option<u64> {
        if self.loading {
            return None;
        }
        let offset = self.page_offset(previous)?;
        self.loading = true;
        self.previous_request = previous;
        Some(offset)
    }

    pub(crate) fn read_request(&self, offset: u64) -> Option<(String, usize)> {
        let id = self.tool.output_id.clone()?;
        let limit = self
            .tool
            .output_range
            .as_ref()
            .map_or(OUTPUT_PAGE_BYTES, |range| {
                range
                    .end
                    .saturating_sub(offset)
                    .min(OUTPUT_PAGE_BYTES as u64) as usize
            });
        Some((id, limit))
    }

    pub(crate) fn retry(&mut self) -> Option<u64> {
        if self.loading || self.tool.output_id.is_none() {
            return None;
        }
        self.loading = true;
        self.previous_request = false;
        Some(self.chunk.as_ref().map_or_else(
            || {
                self.tool
                    .output_range
                    .as_ref()
                    .map_or(0, |range| range.start)
            },
            |chunk| chunk.offset,
        ))
    }

    pub(crate) fn page_size(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    pub(crate) fn render_lines(&mut self, width: usize, height: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let first_page = self.chunk.as_ref().is_none_or(|chunk| chunk.offset == 0);
        if first_page
            && !self.tool.invocation.is_empty()
            && self.tool.invocation != self.tool.summary
        {
            let invocation = render_literal(&self.tool.invocation, Style::default().fg(ACCENT));
            if self.tool.output_id.is_some() {
                lines.extend(wrap_bounded(invocation, width, RETAINED_INVOCATION_ROWS));
            } else {
                lines.extend(invocation);
            }
        }
        if !self.tool.summary.is_empty() {
            lines.extend(render_literal(&self.tool.summary, Style::default()));
        }
        if first_page && !self.tool.evidence.is_empty() {
            let evidence = render_literal(&self.tool.evidence, Style::default().fg(MUTED));
            if self.tool.output_id.is_some() {
                lines.extend(wrap_bounded(evidence, width, RETAINED_EVIDENCE_ROWS));
            } else {
                lines.extend(evidence);
            }
        }
        lines.extend(self.output_lines(width));
        if self.loading {
            lines.push(Line::styled(
                "Reading saved output…",
                Style::default().fg(MUTED),
            ));
        }
        let lines = wrap(lines, width);
        self.height = height;
        self.row_count = lines.len();
        self.top = self.top.min(lines.len().saturating_sub(height));
        lines.into_iter().skip(self.top).take(height).collect()
    }

    pub(crate) fn output_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(chunk) = &self.chunk {
            for segment in &chunk.segments {
                lines.extend(render_literal(&segment.text, stream_style(segment.stream)));
            }
        } else if let Some(formatted) = self.tool.formatted_lines(width) {
            lines.extend(formatted);
        } else if !self.tool.diff.is_empty() {
            lines.extend(render_diff(&self.tool.diff));
        } else if !self.tool.details.is_empty() && self.tool.details != self.tool.summary {
            lines.extend(render_literal(
                &self.tool.details,
                Style::default().fg(Color::Reset),
            ));
        }
        lines
    }

    pub(crate) fn numbered_output_lines(&self, width: usize, total: usize) -> Vec<Line<'static>> {
        let lines = if let Some(chunk) = &self.chunk {
            let mut lines = vec![Line::default()];
            for segment in &chunk.segments {
                let mut rows =
                    render_literal(&segment.text, stream_style(segment.stream)).into_iter();
                if let Some(first) = rows.next() {
                    lines.last_mut().expect("output line").extend(first.spans);
                }
                lines.extend(rows);
            }
            if lines
                .last()
                .is_some_and(|line| line.spans.iter().all(|span| span.content.is_empty()))
            {
                lines.pop();
            }
            lines
        } else {
            self.output_lines(width)
        };
        wrap_numbered(lines, width, usize::MAX, self.first_line, total)
    }

    pub(crate) fn footer(&self) -> Line<'static> {
        if let Some(error) = &self.problem {
            return Line::styled(crate::text::single_line(error), Style::default().fg(ERROR));
        }
        let Some(chunk) = &self.chunk else {
            let name = if self.tool.output_id.is_some() {
                "S Name · "
            } else {
                ""
            };
            return Line::styled(
                format!(
                    "{name}{}/{} Scroll · {} Close",
                    Shortcut::PAGE_UP.label,
                    Shortcut::PAGE_DOWN.label,
                    Shortcut::ESCAPE.label,
                ),
                Style::default().fg(MUTED),
            );
        };
        Line::styled(
            format!(
                "{} · {} Name · {}/{} · {} Refresh · {}",
                page_caption(chunk),
                Shortcut::NAME_OUTPUT.label,
                Shortcut::PAGE_UP.label,
                Shortcut::PAGE_DOWN.label,
                Shortcut::REFRESH_OUTPUT.label,
                Shortcut::ESCAPE.label,
            ),
            Style::default().fg(ACCENT),
        )
    }

    pub(crate) fn page_caption(&self) -> Option<String> {
        self.chunk.as_ref().map(page_caption)
    }
}

fn stream_style(stream: OutputStream) -> Style {
    Style::default().fg(match stream {
        OutputStream::Stdout => Color::Reset,
        OutputStream::Stderr => WARNING,
    })
}

fn page_caption(chunk: &OutputChunk) -> String {
    let mut text = format!(
        "{}–{} / {} bytes",
        chunk.offset, chunk.next_offset, chunk.total_bytes
    );
    if chunk.capture_truncated {
        text.push_str(" · capture truncated");
    }
    if chunk.cleanup_incomplete {
        text.push_str(" · cleanup incomplete");
    } else if !chunk.complete {
        text.push_str(" · capturing");
    }
    if chunk.lossy_utf8 {
        text.push_str(" · decoding replaced bytes");
    }
    text
}

#[cfg(test)]
mod tests {
    use kuku_client::{OutputChunk, OutputSegment, OutputStream};

    use super::{OutputViewer, ToolView};

    #[test]
    fn long_invocation_leaves_room_for_retained_output_and_its_next_page() {
        let mut viewer = OutputViewer::new(
            1,
            ToolView {
                name: "run_command".into(),
                summary: "Finished".into(),
                invocation: format!("long_command {}", "argument ".repeat(240)),
                details: String::new(),
                diff: String::new(),
                output_id: Some("saved".into()),
                evidence: String::new(),
                inline_output_complete: false,
                ..ToolView::default()
            },
        );
        viewer.apply(output(0, "first page", true));
        let first = rendered(&mut viewer);
        assert!(first.contains("long_command"));
        assert!(first.contains("first page"));
        assert_eq!(Some(10), viewer.scroll(1));

        viewer.apply(output(10, "next page!", false));
        let next = rendered(&mut viewer);
        assert!(next.contains("next page!"));
        assert!(!next.contains("long_command"));
        assert_eq!(Some(0), viewer.scroll(-1));
    }

    #[test]
    fn recorded_read_retries_its_offset_and_stops_at_the_range_end() {
        let mut viewer = OutputViewer::new(
            1,
            ToolView {
                output_id: Some("saved".into()),
                output_range: Some(9..15),
                ..ToolView::default()
            },
        );
        viewer.loading = false;
        assert_eq!(Some(9), viewer.retry());
        viewer.apply(output(9, "screen", true));
        assert!(rendered(&mut viewer).contains("screen"));
        assert_eq!(None, viewer.scroll(1));
    }

    fn rendered(viewer: &mut OutputViewer) -> String {
        viewer
            .render_lines(28, 8)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn output(offset: u64, text: &str, has_more: bool) -> OutputChunk {
        OutputChunk {
            output_id: "saved".into(),
            offset,
            next_offset: offset + text.len() as u64,
            total_bytes: 20,
            segments: vec![OutputSegment {
                stream: OutputStream::Stdout,
                offset,
                bytes: text.len(),
                text: text.into(),
                raw_bytes: None,
            }],
            summary: Vec::new(),
            complete: true,
            capture_truncated: false,
            cleanup_incomplete: false,
            filtered: false,
            lossy_utf8: false,
            has_more,
        }
    }
}
