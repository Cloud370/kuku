use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::markup::{render_literal, render_markdown};
use crate::text::{cap, shorten, single_line};
use crate::theme::{
    emphasis, ACTIVITY_CARD_WIDTH, ACTIVITY_MINT, MUTED, THINKING_ROWS, THINKING_SURFACE,
};

use super::{
    append_bounded, render_record, render_rows, Body, Key, RenderCache, Row, Transcript,
    DISPLAY_LIMIT_NOTICE, MAX_BODY_BYTES, MAX_VISUAL_ROWS,
};

impl Transcript {
    pub(crate) fn replace_reply_preview(
        &mut self,
        execution: u64,
        turn: Option<u64>,
        turn_number: Option<u64>,
        mut text: String,
        truncated: bool,
    ) {
        if self.current_execution != Some(execution) {
            self.clear_preview();
            self.current_execution = Some(execution);
        }
        if text.is_empty() {
            self.clear_reply_preview();
            return;
        }
        if text.len() > MAX_BODY_BYTES || truncated {
            text.truncate(text.floor_char_boundary(MAX_BODY_BYTES.min(text.len())));
            text.push_str(DISPLAY_LIMIT_NOTICE);
        }
        let key = Key::Preview(execution);
        let record = self
            .recent
            .record_or_insert(key, turn, turn_number, || Body::Assistant(String::new()));
        if !matches!(&record.body, Body::Assistant(previous) if previous == &text) {
            record.body = Body::Assistant(text);
            record.rendered = RenderCache::default();
            self.mark_unseen();
        }
    }

    pub(crate) fn clear_reply_preview(&mut self) {
        if let Some(execution) = self.current_execution {
            self.recent.remove(&Key::Preview(execution));
        }
    }

    pub(crate) fn replace_summary(
        &mut self,
        execution: u64,
        turn: Option<u64>,
        number: Option<u64>,
        text: &str,
        truncated: bool,
    ) {
        let mut prefix = super::capped(Some(text), MAX_BODY_BYTES);
        if truncated && !prefix.ends_with(DISPLAY_LIMIT_NOTICE) {
            prefix.push_str(DISPLAY_LIMIT_NOTICE);
        }
        if prefix.is_empty() && !self.recent.contains(&Key::Preview(execution)) {
            return;
        }
        let record = self
            .recent
            .record_or_insert(Key::Preview(execution), turn, number, || {
                Body::Assistant(String::new())
            });
        if record.reasoning_preview != prefix {
            record.reasoning_preview = prefix;
            record.rendered = RenderCache::default();
            self.mark_unseen();
        }
    }

    pub(super) fn update_summary(
        &mut self,
        execution: u64,
        turn: Option<u64>,
        number: Option<u64>,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        let record = self
            .recent
            .record_or_insert(Key::Preview(execution), turn, number, || {
                Body::Assistant(String::new())
            });
        if append_bounded(&mut record.reasoning_preview, text, MAX_BODY_BYTES) {
            record.rendered = RenderCache::default();
            self.mark_unseen();
        }
    }
}

pub(super) fn render_assistant(
    text: &str,
    summary: &str,
    width: usize,
    expanded: bool,
) -> (Vec<Row>, usize) {
    let mut rows = Vec::new();
    if !summary.trim().is_empty() {
        let width = width.min(ACTIVITY_CARD_WIDTH);
        let marker = if expanded { "▾" } else { "▸" };
        let mut title = Line::from(Span::styled(
            shorten(&format!("{marker} Thinking preview"), width),
            emphasis(ACTIVITY_MINT),
        ));
        if !expanded && width > title.width() + 4 {
            let excerpt = single_line(cap(summary, 512));
            title.push_span(Span::styled(
                format!(
                    "  {}",
                    shorten(&excerpt, (width - title.width() - 2).min(39))
                ),
                Style::new().fg(MUTED),
            ));
        }
        rows.push(Row::plain(title));
        if expanded {
            let prefix = cap(summary, 4096);
            let inset = super::body_inset(width);
            rows.extend(render_rows(
                render_literal(prefix, Style::new().fg(MUTED)),
                width,
                THINKING_ROWS,
                inset,
            ));
            let more = rows.len() > THINKING_ROWS + 1 || prefix.len() < summary.len();
            rows.truncate(THINKING_ROWS + 1);
            if more {
                rows.push(Row::plain(Line::styled(
                    shorten("  …  Click to fold · /details to read all", width),
                    Style::new().fg(ACTIVITY_MINT),
                )));
            }
        }
        super::paint_card(&mut rows, width, THINKING_SURFACE);
    }
    let reasoning_rows = rows.len();
    if !text.trim().is_empty() {
        rows.extend(render_record(
            if width >= 9 { "Assistant" } else { "AI" },
            MUTED,
            render_markdown(text, width, MAX_VISUAL_ROWS),
            width,
            0,
        ));
    }
    (rows, reasoning_rows)
}
