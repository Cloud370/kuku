use super::{code_surface, emphasis, MUTED};
use crate::text::shorten;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
const MIN_FRAME_WIDTH: usize = 4;
const PADDED_FRAME_WIDTH: usize = 8;

pub(crate) struct CardFrame {
    width: usize,
    bordered: bool,
    padding: usize,
    surface: Style,
}

impl CardFrame {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            width,
            surface: code_surface(),
            bordered: width >= MIN_FRAME_WIDTH,
            padding: usize::from(width >= PADDED_FRAME_WIDTH),
        }
    }

    pub(crate) fn agent(width: usize) -> Self {
        Self {
            surface: code_surface().bg(super::AGENT_SURFACE),
            ..Self::new(width)
        }
    }

    fn edge(&self) -> Style {
        self.surface.fg(MUTED)
    }

    pub(crate) fn bordered(&self) -> bool {
        self.bordered
    }

    pub(crate) fn content_width(&self) -> usize {
        self.width - 2 * self.content_inset()
    }

    pub(crate) fn content_inset(&self) -> usize {
        usize::from(self.bordered) + self.padding
    }

    pub(crate) fn title(&self, label: &str, color: Color) -> Line<'static> {
        self.caption(label, '╭', '╮', self.surface.patch(emphasis(color)))
    }

    pub(crate) fn status_foot(&self, label: &str, color: Color) -> Line<'static> {
        self.caption(label, '╰', '╯', self.surface.fg(color))
    }

    pub(crate) fn foot(&self, label: &str) -> Line<'static> {
        self.caption(label, '╰', '╯', self.surface.fg(MUTED))
    }

    pub(crate) fn divider(&self, label: &str) -> Line<'static> {
        self.caption(label, '├', '┤', self.surface.fg(MUTED))
    }

    fn caption(&self, label: &str, left: char, right: char, style: Style) -> Line<'static> {
        if !self.bordered {
            return Line::from(Span::styled(shorten(label, self.width), style));
        }
        let (prefix, tail) = if self.padding > 0 {
            (format!("{left}─ "), " ")
        } else {
            (left.to_string(), "")
        };
        let room = self.width - prefix.width() - tail.width() - 1;
        let label = shorten(label, room);
        let fill = "─".repeat(room - label.width());
        Line::from(vec![
            Span::styled(prefix, self.edge()),
            Span::styled(label, style),
            Span::styled(format!("{tail}{fill}{right}"), self.edge()),
        ])
    }

    pub(crate) fn line(&self, line: Line<'static>) -> Line<'static> {
        let room = self.content_width().saturating_sub(line.width());
        let mut spans = Vec::with_capacity(line.spans.len() + 4);
        if self.bordered {
            spans.push(Span::styled("│", self.edge()));
        }
        if self.padding != 0 {
            spans.push(Span::styled(" ", self.surface));
        }
        let base = self.surface.patch(line.style);
        spans.extend(line.spans.into_iter().map(|mut span| {
            span.style = base.patch(span.style);
            span
        }));
        if room + self.padding != 0 {
            spans.push(Span::styled(" ".repeat(room + self.padding), self.surface));
        }
        if self.bordered {
            spans.push(Span::styled("│", self.edge()));
        }
        Line::from(spans)
    }

    pub(crate) fn border(&self, left: char, right: char) -> Line<'static> {
        Line::styled(
            format!("{left}{}{right}", "─".repeat(self.width - 2)),
            self.edge(),
        )
    }
}
