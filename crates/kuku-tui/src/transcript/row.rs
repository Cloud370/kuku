use std::borrow::Cow;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::markup::LinkDestination;
use crate::selection::TextInsets;
use crate::text::shorten;
use crate::theme::{ACCENT, CardFrame, MUTED, TURN_RULE, TranscriptTone, tool_operation_color};

use super::body_inset;
use super::details::{InlineOutputAction, OutputTarget, ScriptDetail};

#[derive(Debug, Clone)]
pub(super) struct Row {
    pub(super) line: Line<'static>,
    pub(super) inset: u16,
    pub(super) right_inset: u16,
    pub(super) links: Vec<LinkDestination>,
    pub(super) script_detail: Option<ScriptDetail>,
    pub(super) output: Option<OutputTarget>,
    pub(super) command_arguments: Option<bool>,
    pub(super) inline_output: Option<(OutputTarget, InlineOutputAction)>,
}

impl Row {
    pub(super) fn card(rows: Vec<Self>, frame: &CardFrame) -> Vec<Self> {
        let Some(first) = rows.iter().position(|row| !row.line.spans.is_empty()) else {
            return rows;
        };
        let last = rows
            .iter()
            .rposition(|row| !row.line.spans.is_empty())
            .expect("card has content");
        rows.into_iter()
            .take(last + 1)
            .enumerate()
            .map(|(index, row)| {
                if index == first {
                    row.framed_title(frame)
                } else if index > first {
                    row.framed(frame)
                } else {
                    row
                }
            })
            .chain(Self::frame_end(frame))
            .collect()
    }

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
            TranscriptTone::Text.color(),
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

    pub(super) fn open_output(target: OutputTarget, width: usize) -> Self {
        let mut row = Self::linked_label(target.label(), width);
        row.output = Some(target);
        row
    }

    pub(super) fn inline_output(
        target: OutputTarget,
        action: InlineOutputAction,
        width: usize,
    ) -> Self {
        let mut row = Self::linked_label(action.label(), width);
        row.inline_output = Some((target, action));
        row
    }

    pub(super) fn command_arguments(open: bool, width: usize) -> Self {
        let mut row = Self::linked_label(Self::arguments_label(open), width);
        row.command_arguments = Some(open);
        row
    }

    pub(super) fn arguments_label(open: bool) -> &'static str {
        if open {
            "Hide arguments"
        } else {
            "View arguments"
        }
    }

    pub(super) fn linked_label(text: &str, width: usize) -> Self {
        let label = Span::styled(shorten(text, width), Style::new().fg(ACCENT).underlined());
        Self::plain(Line::from(label))
    }

    pub(super) fn metric(
        label: impl Into<Cow<'static, str>>,
        value: String,
        unit: &'static str,
        tone: TranscriptTone,
    ) -> Line<'static> {
        let muted = TranscriptTone::Label.style();
        Line::from(vec![
            Span::styled(label, muted),
            Span::styled(value, tone.emphasized()),
            Span::styled(unit, muted),
        ])
    }

    pub(super) fn styled_status(
        label: &str,
        status: Line<'static>,
        label_color: Color,
        width: usize,
    ) -> Vec<Self> {
        let mut heading = Line::styled(shorten(label, width), Style::new().fg(label_color));
        if heading.width() + status.width() + 3 <= width {
            heading.push_span(Span::styled(" · ", TranscriptTone::Label.style()));
            heading.extend(status.spans);
            return vec![Self::plain(heading)];
        }
        vec![Self::plain(heading), Self::plain(Self::fit(status, width))]
    }

    pub(super) fn activity_title(verb: &str, color: Color, title: &str, width: usize) -> Self {
        let prefix = if title.is_empty() {
            verb.to_owned()
        } else {
            format!("{verb:<9}")
        };
        let prefix = shorten(&prefix, width);
        let room = width.saturating_sub(prefix.width());
        Self::plain(Line::from(vec![
            Span::styled(prefix, Style::new().fg(color)),
            Span::styled(shorten(title, room), TranscriptTone::Text.style()),
        ]))
    }

    pub(super) fn activity_columns(
        verb: &str,
        verb_color: Color,
        target: &str,
        outcome: Line<'static>,
        width: usize,
    ) -> Self {
        let divider = " · ";
        let prefix = format!("{verb:<9}");
        let available = width.saturating_sub(prefix.width() + divider.width());
        if available < 10 {
            return Self::caption(verb, &outcome.to_string(), verb_color, width);
        }
        let outcome = Self::fit(outcome, available / 2);
        let target = shorten(target, available.saturating_sub(outcome.width()));
        let mut line = Line::from(vec![
            Span::styled(prefix, Style::new().fg(verb_color)),
            Span::raw(target),
            Span::styled(divider, TranscriptTone::Label.style()),
        ]);
        line.extend(outcome.spans);
        Self::plain(line)
    }

    pub(super) fn fit(mut line: Line<'static>, width: usize) -> Line<'static> {
        if width == 0 {
            line.spans.clear();
        } else if line.width() <= width {
            return line;
        } else {
            let mut room = shorten(&line.to_string(), width).len() - '…'.len_utf8();
            let mut truncated = false;
            line.spans.retain_mut(|span| {
                if truncated {
                    return false;
                }
                if span.content.len() <= room {
                    room -= span.content.len();
                } else {
                    span.content = Cow::Owned(format!("{}…", &span.content[..room]));
                    truncated = true;
                }
                true
            });
        }
        line
    }

    pub(super) fn read(path: &str, summary: Line<'static>, width: usize) -> Self {
        Self::action("Read ", path, "", tool_operation_color("read_file"), width)
            .with_summary(summary, width)
    }

    pub(super) fn with_summary(mut self, summary: Line<'static>, width: usize) -> Self {
        let room = width.saturating_sub(self.line.width());
        if room > 4 && summary.width() > 0 {
            self.line
                .push_span(Span::styled(" · ", TranscriptTone::Label.style()));
            let style = summary.style;
            self.line.extend(
                Self::fit(summary, room - 3)
                    .spans
                    .into_iter()
                    .map(|mut span| {
                        span.style = style.patch(span.style);
                        span
                    }),
            );
        }
        self
    }

    pub(super) fn padding(line: Line<'static>, width: usize) -> Self {
        Self {
            line,
            inset: width as u16,
            right_inset: 0,
            links: Vec::new(),
            script_detail: None,
            output: None,
            command_arguments: None,
            inline_output: None,
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

    pub(super) fn framed_title(self, frame: &CardFrame) -> Self {
        self.framed_caption(frame, CardFrame::title_line)
    }

    pub(super) fn framed_footer(self, frame: &CardFrame) -> Self {
        self.framed_caption(frame, CardFrame::foot_line)
    }

    fn framed_caption(
        mut self,
        frame: &CardFrame,
        render: fn(&CardFrame, Line<'static>) -> Line<'static>,
    ) -> Self {
        let text_width = self.line.width();
        let inset = frame.content_inset() + usize::from(frame.bordered());
        self.line = render(frame, self.line);
        self.inset += inset as u16;
        self.right_inset = self.line.width().saturating_sub(text_width + inset) as u16;
        self
    }

    pub(super) fn framed(self, frame: &CardFrame) -> Self {
        let mut row = self;
        let inset = frame.content_inset();
        row.line = frame.line(row.line);
        row.inset += inset as u16;
        row.right_inset += inset as u16;
        for link in &mut row.links {
            link.columns = link.columns.start + inset..link.columns.end + inset;
        }
        row
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

    pub(super) fn detail_width(width: usize) -> usize {
        width.saturating_sub(2 * usize::from(body_inset(width)) + Self::detail_rule(width).width())
    }

    pub(super) fn with_detail_rule(mut self, width: usize) -> Self {
        let rule = Self::detail_rule(width);
        if !rule.is_empty() {
            let rule_width = rule.width();
            self.line
                .spans
                .insert(0, Span::styled(rule, Style::new().fg(TURN_RULE)));
            self.inset += rule_width as u16;
            for link in &mut self.links {
                link.columns = link.columns.start + rule_width..link.columns.end + rule_width;
            }
        }
        self.indented(body_inset(width))
    }

    fn detail_rule(width: usize) -> &'static str {
        if body_inset(width) > 0 { "│ " } else { "" }
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

    pub(super) fn text_insets(&self) -> TextInsets {
        TextInsets {
            left: self.inset,
            right: self.right_inset,
        }
    }

    pub(super) fn plain(line: Line<'static>) -> Self {
        Self {
            line,
            inset: 0,
            right_inset: 0,
            links: Vec::new(),
            script_detail: None,
            output: None,
            command_arguments: None,
            inline_output: None,
        }
    }
}
