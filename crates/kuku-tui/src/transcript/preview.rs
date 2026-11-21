use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::text::shorten;
use crate::theme::{card_padding, emphasis, MUTED, SURFACE_TEXT};

use super::{
    append_bounded, body_inset, render_rows, Body, Key, RenderCache, Row, Transcript,
    DISPLAY_LIMIT_NOTICE, MAX_BODY_BYTES, MAX_VISUAL_ROWS,
};

#[derive(Debug)]
pub(super) struct ReplyPosition {
    pub(super) key: Key,
    pub(super) top: super::Anchor,
}

impl Transcript {
    pub(crate) fn live(
        &mut self,
        execution: u64,
        turn: Option<u64>,
        turn_number: Option<u64>,
        event: &Value,
    ) {
        if self.current_execution != Some(execution) {
            self.reply_position = None;
            self.clear_preview();
            self.current_execution = Some(execution);
        }
        let count = self.recent.records.len();
        match event["type"].as_str() {
            Some("text") => {
                if let Some(text) = event["content"].as_str() {
                    let key = Key::Preview(execution);
                    let record = self.recent.record_or_insert(key, turn, turn_number, || {
                        Body::Assistant(String::new())
                    });
                    if let Body::Assistant(body) = &mut record.body {
                        if append_bounded(body, text, MAX_BODY_BYTES) {
                            record.rendered.preview = None;
                            record.rendered.source = None;
                            self.mark_unseen();
                        }
                    }
                }
            }
            Some("thinking") => self.begin_thinking(execution, turn, turn_number),
            Some("reasoning_preview") => {
                if let Some(text) = event["content"].as_str() {
                    self.update_summary(execution, turn, turn_number, text);
                }
            }
            Some("reply_preview") => {
                if let Some(text) = event["content"].as_str() {
                    self.replace_reply_preview(
                        execution,
                        turn,
                        turn_number,
                        text.to_owned(),
                        event["truncated"].as_bool().unwrap_or_default(),
                    );
                }
                if event["phase"] == "thinking" {
                    self.begin_thinking(execution, turn, turn_number);
                }
                if let Some(text) = event["reasoning_preview"].as_str() {
                    self.replace_summary(
                        execution,
                        turn,
                        turn_number,
                        text,
                        event["reasoning_preview_truncated"] == true,
                    );
                }
            }
            Some("model_request" | "model_recovery") => {
                self.reply_position = None;
                self.clear_reply_preview();
            }
            Some("tool_start" | "tool_output" | "tool_end") => {
                if let Some(id) = event["id"].as_str() {
                    let key = Key::Tool {
                        turn,
                        id: id.to_owned(),
                    };
                    if let Some(reading) = &mut self.reading {
                        if reading.window.contains(&key) {
                            reading
                                .window
                                .tool_live(key.clone(), turn, turn_number, event);
                        }
                    }
                    self.recent.tool_live(key, turn, turn_number, event);
                    self.mark_unseen();
                }
            }
            Some("done" | "cancelled" | "error") => self.clear_preview(),
            Some(_) | None => {}
        }
        if self.recent.records.len() > count {
            self.recent.limit_live();
        }
    }

    pub(super) fn settle_reply_position(&mut self, key: &Key, settled: Option<&super::Record>) {
        if let Some(position) = self.reply_position.as_mut().filter(|p| p.key == *key) {
            if let Some(record) = settled
                .filter(|r| matches!(&r.body, Body::Assistant(text) if !text.trim().is_empty()))
            {
                if position.top.key == *key {
                    position.top.key = record.key.clone();
                }
                position.key = record.key.clone();
            } else {
                self.reply_position = None;
            }
        }
    }

    pub(super) fn keep_first_reply_line(&mut self, width: usize, height: usize) {
        if self.reading.is_some() {
            self.reply_position = None;
            return;
        }
        let Some(position) = &self.reply_position else {
            return;
        };
        if !self.recent.records.iter().any(|r| {
            r.key == position.key && matches!(&r.body, Body::Assistant(t) if !t.trim().is_empty())
        }) {
            return;
        }
        let position = self
            .reply_position
            .take()
            .expect("visible thinking position");
        let layout = super::Layout {
            width,
            tools: self.details,
            assistant: self.assistant_view,
            agents: &self.agent_cards,
            items: &self.detail_items,
        };
        let answer = super::Anchor {
            key: position.key,
            line: super::AnchorLine::Body(1),
        };
        if self.recent.row_count(layout).saturating_sub(height)
            > self.recent.position_of(&answer, layout)
        {
            self.reading = Some(super::Reading {
                window: self.recent.clone(),
                anchor: position.top,
                newer: Default::default(),
                unseen: true,
                manual: false,
            });
        }
    }

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

    pub(crate) fn begin_thinking(
        &mut self,
        execution: u64,
        turn: Option<u64>,
        number: Option<u64>,
    ) {
        self.recent
            .record_or_insert(Key::Preview(execution), turn, number, || {
                Body::Assistant(String::new())
            });
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

pub(super) fn render_assistant(
    text: &str,
    has_thinking: bool,
    live: bool,
    width: usize,
) -> Vec<Row> {
    let has_reply = !text.trim().is_empty();
    let mut rows = assistant_header(has_thinking, has_reply, live, width);
    if has_reply {
        let (mut lines, links) =
            crate::markup::linked_markdown(text, assistant_width(width), MAX_VISUAL_ROWS);
        if lines.last().is_some_and(|line| !line.spans.is_empty()) {
            lines.push(Line::default());
        }
        rows.extend(assistant_body(lines, &links, width, MAX_VISUAL_ROWS));
    }
    rows
}

pub(super) fn assistant_width(width: usize) -> usize {
    width.saturating_sub(usize::from(body_inset(width)) + usize::from(card_padding(width).right))
}

pub(super) fn assistant_body<'a>(
    lines: Vec<Line<'static>>,
    links: &'a crate::markup::LinkLayout,
    width: usize,
    limit: usize,
) -> impl Iterator<Item = Row> + 'a {
    render_rows(lines, width, limit, body_inset(width)).map(|mut row| {
        row.links = links.resolve(&mut row.line);
        row
    })
}

pub(super) fn assistant_header(
    has_thinking: bool,
    has_reply: bool,
    live: bool,
    width: usize,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let role = if width >= 9 { "Assistant" } else { "AI" };
    let placeholder = live && !has_reply;
    let joined = has_thinking && width >= 27;
    if has_thinking {
        let mut title = Line::default();
        if joined {
            title.push_span(Span::styled(format!("{role}   "), emphasis(SURFACE_TEXT)));
        }
        title.push_span(Span::styled(
            shorten("View thinking ›", width.saturating_sub(title.width())),
            emphasis(crate::theme::MODEL),
        ));
        rows.push(Row::plain(title));
    }
    if placeholder {
        if !has_thinking {
            rows.push(Row::plain(Line::styled(role, emphasis(SURFACE_TEXT))));
        }
        rows.push(Row::plain(Line::styled("Thinking", Style::new().fg(MUTED))));
        rows.push(Row::plain(Line::default()));
    } else if has_reply && !joined {
        let title = Line::styled(role, emphasis(SURFACE_TEXT));
        rows.extend(render_rows(vec![title], width, MAX_VISUAL_ROWS, 0));
    }
    rows
}
