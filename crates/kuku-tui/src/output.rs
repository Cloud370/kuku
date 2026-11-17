use kuku_client::{OutputChunk, OutputStream};
use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::markup::{render_diff, render_literal, wrap, ACCENT, MUTED};
use crate::transcript::ToolView;

pub(crate) const OUTPUT_PAGE_BYTES: usize = 4 * 1024;
const MAX_PREVIOUS_OFFSETS: usize = 128;

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
    offsets: Vec<u64>,
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
            previous_request: false,
        }
    }

    pub(crate) fn apply(&mut self, chunk: OutputChunk) {
        self.loading = false;
        self.problem = None;
        if let Some(previous) = &self.chunk {
            if self.previous_request {
                self.offsets.pop();
            } else if chunk.offset > previous.offset {
                if self.offsets.len() == MAX_PREVIOUS_OFFSETS {
                    self.offsets.remove(0);
                }
                self.offsets.push(previous.offset);
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
        let chunk = self.chunk.as_ref()?;
        if self.loading {
            return None;
        }
        let offset = if previous {
            self.offsets.last().copied()
        } else if next && (chunk.has_more || !chunk.complete) {
            Some(chunk.next_offset)
        } else {
            None
        }?;
        self.loading = true;
        self.previous_request = previous;
        Some(offset)
    }

    pub(crate) fn retry(&mut self) -> Option<u64> {
        if self.loading || self.tool.output_id.is_none() {
            return None;
        }
        self.loading = true;
        self.previous_request = false;
        Some(self.chunk.as_ref().map_or(0, |chunk| chunk.offset))
    }

    pub(crate) fn page_size(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    pub(crate) fn render_lines(&mut self, width: usize, height: usize) -> Vec<Line<'static>> {
        let mut lines = render_literal(&self.tool.summary, Style::default());
        if let Some(chunk) = &self.chunk {
            for segment in &chunk.segments {
                let color = match segment.stream {
                    OutputStream::Stdout => Color::Gray,
                    OutputStream::Stderr => Color::Yellow,
                };
                lines.extend(render_literal(&segment.text, Style::default().fg(color)));
            }
        } else if !self.tool.diff.is_empty() {
            lines.extend(render_diff(&self.tool.diff));
        } else if !self.tool.details.is_empty() {
            lines.extend(render_literal(
                &self.tool.details,
                Style::default().fg(Color::Gray),
            ));
        }
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

    pub(crate) fn footer(&self) -> Line<'static> {
        if let Some(error) = &self.problem {
            return Line::styled(
                crate::text::single_line(error),
                Style::default().fg(Color::Red),
            );
        }
        let Some(chunk) = &self.chunk else {
            return Line::styled("PgUp/PgDn Scroll · Esc Close", Style::default().fg(MUTED));
        };
        let state = if chunk.cleanup_incomplete {
            " · cleanup incomplete"
        } else if chunk.capture_truncated {
            " · capture truncated"
        } else if !chunk.complete {
            " · capturing"
        } else {
            ""
        };
        Line::styled(
            format!(
                "{}–{} / {} bytes{state} · PgUp/PgDn · R Refresh · Esc",
                chunk.offset, chunk.next_offset, chunk.total_bytes
            ),
            Style::default().fg(ACCENT),
        )
    }
}
