use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::markup::render_markdown;
use crate::text::shorten;
use crate::theme::{emphasis, MUTED, SURFACE_TEXT};

use super::{
    append_bounded, render_record, Body, Key, RenderCache, Row, Transcript, DISPLAY_LIMIT_NOTICE,
    MAX_BODY_BYTES, MAX_VISUAL_ROWS,
};

impl Transcript {
    pub(crate) fn live_thinking(&self) -> Option<(u64, &str)> {
        let id = self.current_execution?;
        let record = self
            .recent
            .records
            .iter()
            .rev()
            .find(|r| r.key == Key::Preview(id))?;
        (!record.reasoning_preview.is_empty()).then_some((id, record.reasoning_preview.as_str()))
    }

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
            let visibility_changed = record.reasoning_preview.is_empty() != prefix.is_empty();
            record.reasoning_preview = prefix;
            if visibility_changed {
                record.rendered = RenderCache::default();
                self.mark_unseen();
            }
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
        let was_empty = record.reasoning_preview.is_empty();
        if append_bounded(&mut record.reasoning_preview, text, MAX_BODY_BYTES) && was_empty {
            record.rendered = RenderCache::default();
            self.mark_unseen();
        }
    }
}

pub(super) fn render_assistant(text: &str, has_thinking: bool, width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let has_reply = !text.trim().is_empty();
    let role = if width >= 9 { "Assistant" } else { "AI" };
    let joined = has_reply && has_thinking && width >= 28;
    if has_thinking {
        let mut title = Line::default();
        if joined {
            title.push_span(Span::styled(format!("{role}   "), emphasis(SURFACE_TEXT)));
        }
        title.push_span(Span::styled(
            shorten("View thinking", width.saturating_sub(title.width())),
            Style::new().fg(MUTED),
        ));
        rows.push(Row::plain(title));
    }
    if has_reply {
        rows.extend(
            render_record(
                role,
                SURFACE_TEXT,
                render_markdown(text, width, MAX_VISUAL_ROWS),
                width,
                0,
            )
            .into_iter()
            .skip(usize::from(joined)),
        );
    }
    rows
}
