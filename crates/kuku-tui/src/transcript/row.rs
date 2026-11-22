use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::markup::LinkDestination;
use crate::text::shorten;
use crate::theme::{CardFrame, MUTED, SURFACE_TEXT};

#[derive(Debug, Clone)]
pub(super) struct Row {
    pub(super) line: Line<'static>,
    pub(super) inset: u16,
    pub(super) links: Vec<LinkDestination>,
}

impl Row {
    pub(super) fn action(
        prefix: &str,
        label: &str,
        summary: &str,
        color: Color,
        width: usize,
    ) -> Self {
        let prefix = shorten(prefix, width);
        let mut row = Self::caption(
            label,
            summary,
            SURFACE_TEXT,
            width.saturating_sub(prefix.width()),
        );
        row.line
            .spans
            .insert(0, Span::styled(prefix, Style::new().fg(color)));
        row
    }

    pub(super) fn caption(label: &str, summary: &str, color: Color, width: usize) -> Self {
        let mut line = Line::styled(shorten(label, width), Style::new().fg(color));
        if !summary.is_empty() {
            let room = width.saturating_sub(line.width());
            if room > 4 {
                line.push_span(Span::styled(
                    shorten(&format!(" · {summary}"), room),
                    Style::new().fg(MUTED),
                ));
            }
        }
        Self::plain(line)
    }

    pub(super) fn padding(line: Line<'static>, width: usize) -> Self {
        Self {
            line,
            inset: width as u16,
            links: Vec::new(),
        }
    }

    pub(super) fn frame_padding(frame: &CardFrame) -> Option<Self> {
        frame.padding_line().map(|line| {
            let width = line.width();
            Self::padding(line, width)
        })
    }

    pub(super) fn pad_framed_body(rows: &mut Vec<Self>, frame: &CardFrame) {
        if rows.len() > 1 {
            if let Some(padding) = Self::frame_padding(frame) {
                rows.insert(1, padding.clone());
                rows.push(padding);
            }
        }
    }

    pub(super) fn framed(mut self, frame: &CardFrame) -> Self {
        let inset = frame.content_inset();
        self.line = frame.line(self.line);
        self.inset += inset as u16;
        for link in &mut self.links {
            link.columns = link.columns.start + inset..link.columns.end + inset;
        }
        self
    }

    pub(super) fn indented(mut self, inset: u16) -> Self {
        self.inset += inset;
        if inset != 0 && !self.line.spans.is_empty() {
            self.line
                .spans
                .insert(0, Span::raw(" ".repeat(inset.into())));
            for link in &mut self.links {
                let inset = usize::from(inset);
                link.columns = link.columns.start + inset..link.columns.end + inset;
            }
        }
        self
    }

    pub(super) fn with_rail(mut self, inset: u16) -> Self {
        if inset == 0 {
            return self;
        }
        self.line
            .spans
            .insert(0, Span::styled("│ ", Style::new().fg(MUTED)));
        self.inset += inset;
        for link in &mut self.links {
            let inset = usize::from(inset);
            link.columns = link.columns.start + inset..link.columns.end + inset;
        }
        self
    }

    pub(super) fn frame_end(frame: &CardFrame) -> Vec<Self> {
        let mut rows: Vec<_> = Self::frame_padding(frame).into_iter().collect();
        if frame.bordered() {
            let line = frame.border('╰', '╯');
            let width = line.width();
            rows.push(Self::padding(line, width));
        }
        rows.push(Self::plain(Line::default()));
        rows
    }

    pub(super) fn plain(line: Line<'static>) -> Self {
        Self {
            line,
            inset: 0,
            links: Vec::new(),
        }
    }
}
