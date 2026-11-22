use super::{
    append_line_count, card_padding, code_surface, emphasis, transcript_background, TranscriptTone,
    ACCENT, MUTED, TURN_RULE,
};
use crate::text::shorten;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Padding;
use std::ops::Range;
use unicode_width::UnicodeWidthStr;
const MIN_FRAME_WIDTH: usize = 4;

pub(crate) struct CardFrame {
    width: usize,
    bordered: bool,
    padding: Padding,
    surface: Style,
    border_color: Color,
}

impl CardFrame {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            width,
            surface: code_surface(),
            border_color: MUTED,
            bordered: width >= MIN_FRAME_WIDTH,
            padding: card_padding(width),
        }
    }

    pub(crate) fn timeline(width: usize) -> Self {
        Self {
            width,
            surface: TranscriptTone::Text.style().patch(transcript_background()),
            border_color: TURN_RULE,
            bordered: width >= 18,
            padding: Padding::horizontal(u16::from(width >= 12)),
        }
    }

    fn edge(&self) -> Style {
        self.surface.fg(self.border_color)
    }

    pub(crate) fn bordered(&self) -> bool {
        self.bordered
    }

    pub(crate) fn content_width(&self) -> usize {
        self.width - 2 * self.content_inset()
    }

    pub(crate) fn content_inset(&self) -> usize {
        usize::from(self.bordered) + usize::from(self.padding.left)
    }

    pub(crate) fn padding_line(&self) -> Option<Line<'static>> {
        (self.padding.top > 0).then(|| self.line(Line::default()))
    }

    pub(crate) fn title(&self, label: &str, color: Color) -> Line<'static> {
        self.caption(label, '╭', '╮', self.surface.patch(emphasis(color)))
    }

    pub(crate) fn code_title(&self, language: Option<&str>, lines: usize) -> Line<'static> {
        let width = self.title_width().min(self.content_width());
        let label = shorten(language.unwrap_or_default(), width);
        let mut title = Line::styled(label, emphasis(ACCENT));
        append_line_count(&mut title, width, lines);
        if self.bordered && title.width() == 0 {
            return self.border('╭', '╮');
        }
        self.title_line(title)
    }

    pub(crate) fn title_width(&self) -> usize {
        if !self.bordered {
            return self.content_width();
        }
        self.width - if self.padding.left > 0 { 5 } else { 2 }
    }

    pub(crate) fn title_line(&self, label: Line<'static>) -> Line<'static> {
        self.caption_line(label, '╭', '╮')
    }

    pub(crate) fn agent_action(width: usize) -> Option<(&'static str, Range<usize>)> {
        let action = match width {
            0..=17 => return None,
            18..=47 => "Open →",
            _ => "Open agent →",
        };
        let end = width - 2;
        Some((action, end - action.width()..end))
    }

    pub(crate) fn agent_title(&self, label: &str, confirmed: bool) -> Line<'static> {
        let color = super::MODEL;
        let Some((action, columns)) = Self::agent_action(self.width).filter(|_| confirmed) else {
            return self.title(label, color);
        };
        if !self.bordered {
            let space = columns.start.saturating_sub(self.content_inset());
            let label = shorten(label, space.saturating_sub(2));
            let fill = " ".repeat(space.saturating_sub(label.width()));
            return self.line(Line::from(vec![
                Span::styled(label, emphasis(color)),
                Span::raw(fill),
                Span::styled(action, Style::new().fg(color).underlined()),
            ]));
        }
        let label = shorten(label, columns.start.saturating_sub(5));
        let fill = columns.start - 5 - label.width();
        Line::from(vec![
            Span::styled("╭─ ", self.edge()),
            Span::styled(label, self.surface.patch(emphasis(color))),
            Span::styled(format!(" {} ", "─".repeat(fill)), self.edge()),
            Span::styled(action, self.surface.fg(color).underlined()),
            Span::styled(" ╮", self.edge()),
        ])
    }

    pub(crate) fn foot(&self, label: &str) -> Line<'static> {
        self.caption(label, '╰', '╯', self.surface.fg(MUTED))
    }

    pub(crate) fn foot_line(&self, label: Line<'static>) -> Line<'static> {
        self.caption_line(label, '╰', '╯')
    }

    pub(crate) fn divider(&self, label: &str) -> Line<'static> {
        self.caption(label, '├', '┤', self.surface.fg(MUTED))
    }

    fn caption(&self, label: &str, left: char, right: char, style: Style) -> Line<'static> {
        let label = Line::styled(shorten(label, self.title_width()), style);
        self.caption_line(label, left, right)
    }

    fn caption_line(&self, mut label: Line<'static>, left: char, right: char) -> Line<'static> {
        let room = self.title_width();
        debug_assert!(label.width() <= self.content_width());
        let base = self.surface.patch(label.style);
        for span in &mut label.spans {
            span.style = base.patch(span.style);
        }
        label.style = Style::new();
        if !self.bordered {
            return self.line(label);
        }
        let (prefix, tail) = if self.padding.left > 0 {
            (
                format!("{left}─ "),
                if label.width() > room { "" } else { " " },
            )
        } else {
            (left.to_string(), "")
        };
        let fill = "─".repeat(self.width - prefix.width() - tail.width() - label.width() - 1);
        label.spans.insert(0, Span::styled(prefix, self.edge()));
        label.push_span(Span::styled(format!("{tail}{fill}{right}"), self.edge()));
        label
    }

    pub(crate) fn line(&self, line: Line<'static>) -> Line<'static> {
        let room = self.content_width().saturating_sub(line.width());
        let mut spans = Vec::with_capacity(line.spans.len() + 4);
        if self.bordered {
            spans.push(Span::styled("│", self.edge()));
        }
        if self.padding.left != 0 {
            spans.push(Span::styled(
                " ".repeat(usize::from(self.padding.left)),
                self.surface,
            ));
        }
        let base = self.surface.patch(line.style);
        spans.extend(line.spans.into_iter().map(|mut span| {
            span.style = base.patch(span.style);
            span
        }));
        if room + usize::from(self.padding.right) != 0 {
            spans.push(Span::styled(
                " ".repeat(room + usize::from(self.padding.right)),
                self.surface,
            ));
        }
        if self.bordered {
            spans.push(Span::styled("│", self.edge()));
        }
        Line::from(spans)
    }

    pub(crate) fn border(&self, left: char, right: char) -> Line<'static> {
        Line::from(Span::styled(
            format!("{left}{}{right}", "─".repeat(self.width - 2)),
            self.edge(),
        ))
    }
}
