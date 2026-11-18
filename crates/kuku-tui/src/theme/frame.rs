use super::{code_surface, emphasis, CODE_BACKGROUND, MUTED};
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
}

impl CardFrame {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            width,
            bordered: width >= MIN_FRAME_WIDTH,
            padding: usize::from(width >= PADDED_FRAME_WIDTH),
        }
    }

    pub(crate) fn bordered(&self) -> bool {
        self.bordered
    }

    pub(crate) fn content_width(&self) -> usize {
        self.width - 2 * (usize::from(self.bordered) + self.padding)
    }

    pub(crate) fn title(&self, label: &str, color: Color) -> Line<'static> {
        self.caption(label, '╭', '╮', emphasis(color).bg(CODE_BACKGROUND))
    }

    pub(crate) fn foot(&self, label: &str) -> Line<'static> {
        self.caption(label, '╰', '╯', code_surface().fg(MUTED))
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
            Span::styled(prefix, edge()),
            Span::styled(label, style),
            Span::styled(format!("{tail}{fill}{right}"), edge()),
        ])
    }

    pub(crate) fn line(&self, line: Line<'static>) -> Line<'static> {
        let room = self.content_width().saturating_sub(line.width());
        let mut spans = Vec::with_capacity(line.spans.len() + 4);
        if self.bordered {
            spans.push(Span::styled("│", edge()));
        }
        if self.padding != 0 {
            spans.push(Span::styled(" ", code_surface()));
        }
        let base = code_surface().patch(line.style);
        spans.extend(line.spans.into_iter().map(|mut span| {
            span.style = base.patch(span.style);
            span
        }));
        if room + self.padding != 0 {
            spans.push(Span::styled(
                " ".repeat(room + self.padding),
                code_surface(),
            ));
        }
        if self.bordered {
            spans.push(Span::styled("│", edge()));
        }
        Line::from(spans)
    }

    pub(crate) fn border(&self, left: char, right: char) -> Line<'static> {
        Line::styled(
            format!("{left}{}{right}", "─".repeat(self.width - 2)),
            edge(),
        )
    }
}

fn edge() -> Style {
    code_surface().fg(MUTED)
}
