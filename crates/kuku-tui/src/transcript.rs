use std::collections::VecDeque;

use kuku_client::{HistoryEntry, HistoryKind, HistoryPage, TurnSummary};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap_bounded, LinkDestination};
use crate::text::{cap, shorten, single_line};
use crate::theme::{ACCENT, MIN_TRANSCRIPT_INDENT_WIDTH, MUTED, TRANSCRIPT_INDENT, WARNING};

mod agent;
mod body;
mod details;
mod job;
mod message;
mod notes;
mod preview;
mod reply;
mod retry;
mod rewind;
mod row;
use row::Row;
mod tool;
mod turn_summary;
mod window;

use self::agent::AgentCards;
use self::body::Body;
pub(crate) use self::details::DetailKey;
use self::details::{DetailRecord, Details, VisibleDetail};
use self::job::job_notice;
use self::message::{paint_card, render_assistant_source, render_record, render_user};
use self::reply::LatestReply;
pub(crate) use self::reply::ReplySource;
use self::tool::Tool;
pub(crate) use self::tool::ToolView;
use self::window::{Layout, RenderCache, TurnHeading, Window};

pub(crate) const HISTORY_PAGE: usize = 48;
const MAX_RECORDS: usize = HISTORY_PAGE * 3;
const MAX_LIVE_RECORDS: usize = 32;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_BACKTRACK_PAGES: usize = 128;
const MAX_VISUAL_ROWS: usize = 768;
const DISPLAY_LIMIT_NOTICE: &str = "\n… Display limit reached";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Event(u64),
    Preview(u64),
    Tool { turn: Option<u64>, id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Anchor {
    key: Key,
    line: AnchorLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorLine {
    Turn,
    Group(usize),
    Body(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryRequest {
    Recent,
    Older(u64),
    Newer { first: u64, before: u64 },
    Restore(u64),
}

impl HistoryRequest {
    pub(crate) fn before(self) -> u64 {
        match self {
            Self::Recent => 0,
            Self::Older(before) | Self::Newer { before, .. } | Self::Restore(before) => before,
        }
    }

    pub(crate) fn before_option(self) -> Option<u64> {
        (self != Self::Recent).then(|| self.before())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ToolDetails {
    #[default]
    Expanded,
    Collapsed,
    GroupMember,
    Inspection,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum AssistantView {
    #[default]
    Preview,
    Source,
}

#[derive(Debug, Default)]
pub(crate) struct Transcript {
    recent: Window,
    latest_reply: LatestReply,
    reading: Option<Reading>,
    restore: Option<(u64, Anchor)>,
    details: ToolDetails,
    assistant_view: AssistantView,
    current_execution: Option<u64>,
    notes: Option<notes::ThinkingRead>,
    reply_position: Option<preview::ReplyPosition>,
    agent_cards: AgentCards,
    detail_items: Details,
    visible_links: Vec<(usize, LinkDestination)>,
}

impl Transcript {
    pub(crate) fn invalidate_after_rewind(&mut self) {
        self.reply_position = None;
        self.reading = None;
        self.restore = None;
        self.latest_reply = LatestReply::default();
        self.recent = Window::default();
        self.current_execution = None;
        self.detail_items.visible.clear();
    }

    pub(crate) fn replace_recent(&mut self, page: HistoryPage) {
        self.reply_position = None;
        if let Some(reading) = &mut self.reading {
            let previous = self
                .recent
                .records
                .iter()
                .filter_map(|record| record.position)
                .max();
            reading.unseen |= page
                .entries
                .last()
                .is_some_and(|entry| previous.is_some_and(|previous| entry.id > previous));
        }
        self.latest_reply.observe_page(&page);
        self.recent = Window::from_page(page);
        self.current_execution = None;
        self.detail_items.visible.clear();
    }

    pub(crate) fn suspend(&mut self) {
        self.reply_position = None;
        self.notes = None;
        if let Some(reading) = self.reading.take() {
            self.restore = reading.restore_at();
        }
        self.recent = Window::default();
        self.current_execution = None;
        self.detail_items.visible.clear();
    }

    pub(crate) fn restore_request(&self) -> Option<HistoryRequest> {
        self.restore
            .as_ref()
            .map(|(before, _)| HistoryRequest::Restore(*before))
    }

    pub(crate) fn apply(&mut self, entry: HistoryEntry, execution: Option<u64>) {
        self.latest_reply.observe(&entry);
        let entry_turn = entry.turn;
        let error_position = (entry.kind == HistoryKind::Error).then_some((entry.id, entry_turn));
        let interrupted_execution = retry::interruption_execution(&entry);
        let previous_preview = if entry.kind == HistoryKind::Assistant {
            execution.or(self.current_execution)
        } else {
            interrupted_execution
        };
        let mut record = Record::from_entry(entry);
        if let (Some(record), Some(execution)) = (&mut record, execution) {
            record.execution_id.get_or_insert(execution);
        }
        self.recent
            .preserve_interrupted_reply(&mut record, interrupted_execution, entry_turn);
        if let (Some((event, turn)), Some(execution)) = (error_position, self.current_execution) {
            self.recent
                .position_preview_before(&Key::Preview(execution), event, turn);
        }
        if let Some(execution) = previous_preview {
            let key = Key::Preview(execution);
            self.settle_reply_position(&key, record.as_ref());
            if let (Some(notes), Some(record)) = (&mut self.notes, &record) {
                notes.key.settle(&key, &record.key);
            }
            self.recent.remove(&key);
            if let Some(reading) = &mut self.reading {
                reading.settle_preview(&key, record.as_ref());
            }
        }
        let Some(record) = record else { return };
        if let Some(reading) = &mut self.reading {
            reading.unseen = true;
            if matches!(record.key, Key::Tool { .. }) && reading.window.contains(&record.key) {
                reading.window.insert(record.clone());
            }
        }
        self.recent.insert(record);
        self.recent.trim_front();
    }

    pub(crate) fn clear_preview(&mut self) {
        self.recent.clear_preview();
        if let Some(reading) = &mut self.reading {
            if reading
                .window
                .records
                .iter()
                .any(|record| record.key == reading.anchor.key && record.position.is_none())
            {
                if let Some(key) = reading
                    .window
                    .records
                    .iter()
                    .rev()
                    .find(|record| record.position.is_some())
                    .map(|record| record.key.clone())
                {
                    reading.anchor = Anchor {
                        key,
                        line: AnchorLine::Body(0),
                    };
                }
            }
            reading.window.clear_preview();
        }
        self.current_execution = None;
    }

    pub(crate) fn show_latest(&mut self) {
        self.reply_position = None;
        self.notes = None;
        self.reading = None;
        self.restore = None;
    }

    pub(crate) fn reading_anchor(&self) -> Option<&Anchor> {
        self.reading.as_ref().map(|reading| &reading.anchor)
    }

    pub(crate) fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    pub(crate) fn reading_has_new_content(&self) -> Option<bool> {
        self.reading.as_ref().map(|reading| reading.unseen)
    }

    pub(crate) fn history_hint(&self) -> Option<bool> {
        self.reading
            .as_ref()
            .filter(|reading| reading.manual && self.notes.is_none())
            .map(|reading| reading.unseen)
    }

    fn mark_unseen(&mut self) {
        if let Some(reading) = &mut self.reading {
            reading.unseen = true;
        }
    }

    pub(crate) fn toggle_details(&mut self) -> bool {
        let expanded = self.details_can_expand();
        self.details = if expanded {
            ToolDetails::Expanded
        } else {
            ToolDetails::Collapsed
        };
        self.agent_cards
            .set_details(expanded, self.detail_items.agent_keys());
        self.toggle_group_details(expanded);
        expanded
    }

    pub(crate) fn toggle_source(&mut self) -> bool {
        self.assistant_view = match self.assistant_view {
            AssistantView::Preview => AssistantView::Source,
            AssistantView::Source => AssistantView::Preview,
        };
        self.assistant_view == AssistantView::Source
    }

    pub(crate) fn last_reply(&self) -> Option<ReplySource<'_>> {
        self.latest_reply.source(&self.recent.records)
    }

    pub(crate) fn tools(&self) -> Vec<ToolView> {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        window
            .records
            .iter()
            .rev()
            .filter_map(|record| match &record.body {
                Body::Tool(tool) => Some(tool),
                Body::User { .. }
                | Body::Assistant(_)
                | Body::Interrupted(_)
                | Body::Notice(..)
                | Body::ModelError { .. }
                | Body::Retry { .. }
                | Body::TurnSummary => None,
            })
            .flat_map(|tool| std::iter::once(tool.view()).chain(tool.output_views()))
            .collect()
    }

    pub(crate) fn scroll(
        &mut self,
        amount: isize,
        width: usize,
        height: usize,
    ) -> Option<HistoryRequest> {
        self.link_recent_agent_cards();
        let layout = Layout {
            width,
            tools: self.details,
            assistant: self.assistant_view,
            agents: &self.agent_cards,
            items: &self.detail_items,
        };
        if self.reading.is_none() {
            if amount > 0 || self.recent.records.is_empty() {
                return None;
            }
            let mut window = self.recent.clone();
            let rows = window.row_count(layout);
            let anchor = window.anchor_at(rows.saturating_sub(height), layout)?;
            self.reading = Some(Reading {
                window,
                anchor,
                newer: VecDeque::new(),
                unseen: false,
                manual: true,
            });
        }
        if amount == 0 {
            return None;
        }
        let reading = self.reading.as_mut().expect("read window");
        reading.manual = true;
        let rows = reading.window.row_count(layout);
        let start = reading.window.position_of(&reading.anchor, layout);
        let bottom = rows.saturating_sub(height);
        let next = start.saturating_add_signed(amount).min(bottom);
        if let Some(anchor) = reading.window.anchor_at(next, layout) {
            reading.anchor = anchor;
        }
        if amount.is_negative() && next == 0 {
            return reading.window.older.map(HistoryRequest::Older);
        }
        if amount > 0 && next == bottom {
            if let Some(request) = reading.newer.front() {
                return Some(*request);
            }
            self.show_latest();
        }
        None
    }

    pub(crate) fn apply_page(&mut self, request: HistoryRequest, page: HistoryPage) {
        if request == HistoryRequest::Recent {
            self.replace_recent(page);
            return;
        }
        if let HistoryRequest::Restore(before) = request {
            if self
                .restore
                .as_ref()
                .is_some_and(|(expected, _)| *expected == before)
            {
                let (_, anchor) = self.restore.take().expect("restoration requested");
                let window = Window::from_page(page);
                if window.contains(&anchor.key) {
                    self.reading = Some(Reading {
                        window,
                        anchor,
                        newer: VecDeque::new(),
                        unseen: false,
                        manual: true,
                    });
                }
            }
            return;
        }
        let Some(reading) = &mut self.reading else {
            return;
        };
        match request {
            HistoryRequest::Older(before) if reading.window.older == Some(before) => {
                reading.window.older = page.next_before;
                for record in page.entries.into_iter().filter_map(Record::from_entry) {
                    reading.window.insert(record);
                }
                if let Some(request) = reading.window.trim_back() {
                    reading.newer.push_front(request);
                    reading.newer.truncate(MAX_BACKTRACK_PAGES);
                }
            }
            HistoryRequest::Newer { first, .. } if reading.newer.front() == Some(&request) => {
                reading.newer.pop_front();
                for record in page
                    .entries
                    .into_iter()
                    .filter(|entry| entry.id >= first)
                    .filter_map(Record::from_entry)
                {
                    reading.window.insert(record);
                }
                reading.window.trim_front();
            }
            HistoryRequest::Older(_)
            | HistoryRequest::Newer { .. }
            | HistoryRequest::Restore(_)
            | HistoryRequest::Recent => {}
        }
    }

    pub(crate) fn project(&mut self, width: usize, height: usize) -> TranscriptView {
        self.link_recent_agent_cards();
        self.keep_first_reply_line(width, height);
        let layout = Layout {
            width,
            tools: self.details,
            assistant: self.assistant_view,
            agents: &self.agent_cards,
            items: &self.detail_items,
        };
        let (window, start, following) = if let Some(reading) = &mut self.reading {
            let start = reading.window.position_of(&reading.anchor, layout);
            (&mut reading.window, start, false)
        } else {
            let rows = self.recent.row_count(layout);
            (&mut self.recent, rows.saturating_sub(height), true)
        };
        let limit = height.saturating_add(usize::from(self.notes.is_some()));
        let mut lines = Vec::with_capacity(limit);
        let mut row_insets = Vec::with_capacity(height);
        self.visible_links.clear();
        let mut visible = Vec::new();
        let mut thinking_row = None;
        let mut skip = start;
        for block in window.blocks(layout) {
            if lines.len() == limit {
                break;
            }
            if skip == 0 && block.height() == 0 {
                if let Some(detail @ DetailRecord::Reasoning(_)) = block.detail {
                    let row = lines.len();
                    visible.push(VisibleDetail::record(row..row, block.key, detail, false));
                }
                continue;
            }
            if skip >= block.height() {
                skip -= block.height();
                continue;
            }
            if block.turn_gap {
                if skip == 0 {
                    lines.push(Line::default());
                    row_insets.push(0);
                }
                skip = skip.saturating_sub(1);
            }
            if let Some(turn) = block.heading {
                if skip == 0 {
                    lines.push(message::render_turn(turn, width, block.user_header));
                    row_insets.push(0);
                }
                skip = skip.saturating_sub(1);
            }
            if let Some(group) = block.group {
                let first = lines.len();
                let title_visible = skip == 0;
                for row in group.rows.iter().skip(skip).take(limit - first) {
                    lines.push(row.line.clone());
                    row_insets.push(row.inset);
                }
                skip = skip.saturating_sub(group.rows.len());
                let end = lines.len()
                    - usize::from(lines.last().is_some_and(|line| line.spans.is_empty()));
                if first < end {
                    visible.push(VisibleDetail::group(
                        first..end,
                        block.key,
                        group,
                        title_visible,
                    ));
                }
            }
            let first = lines.len();
            for row in block.rows.iter().skip(skip).take(limit - lines.len()) {
                self.visible_links
                    .extend(row.links.iter().cloned().map(|link| (lines.len(), link)));
                lines.push(row.line.clone());
                row_insets.push(row.inset);
            }
            if following && block.thinking && skip <= 1 && first + 1 - skip < lines.len() {
                thinking_row = Some(first + 1 - skip);
            }
            if let Some(detail) = block.detail {
                let last = lines.len()
                    - usize::from(
                        lines.len() > first
                            && lines.last().is_some_and(|line| line.spans.is_empty()),
                    );
                let last = if let DetailRecord::Reasoning(_) = detail {
                    last.min(first + usize::from(skip == 0))
                } else {
                    last
                };
                if last > first
                    || (matches!(detail, DetailRecord::Reasoning(_))
                        && block.rows.is_empty()
                        && first < height)
                {
                    visible.push(VisibleDetail::record(
                        first..last,
                        block.key,
                        detail,
                        skip == 0 && !block.grouped,
                    ));
                }
            }
            skip = 0;
        }
        if thinking_row.is_some() && self.reply_position.is_none() {
            if let (Some(execution), Some(top)) =
                (self.current_execution, window.anchor_at(start, layout))
            {
                self.reply_position = Some(preview::ReplyPosition {
                    key: Key::Preview(execution),
                    top,
                });
            }
        }
        self.detail_items.visible = visible;
        let tool_hint = (!self.detail_items.visible.is_empty()).then(|| {
            if self.detail_items.only_reasoning() {
                return "/details".to_owned();
            }
            let action = if self.details_can_expand() {
                "show"
            } else {
                "hide"
            };
            format!("{} {action} · /details", Shortcut::DETAILS.label)
        });
        let mut view = TranscriptView {
            thinking_row,
            lines,
            row_insets,
            following,
            tool_hint,
        };
        if let Some(notes) = &mut self.notes {
            let origin = self.detail_items.rows_for(&notes.key);
            notes.paint(&mut view, origin, width, height);
        }
        view
    }

    fn details_can_expand(&self) -> bool {
        self.details == ToolDetails::Collapsed
            || self.detail_items.has_collapsed_detail()
            || self
                .detail_items
                .agent_keys()
                .any(|key| !self.agent_cards.is_open(key))
    }
}

pub(crate) struct TranscriptView {
    pub(crate) thinking_row: Option<usize>,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) row_insets: Vec<u16>,
    pub(crate) following: bool,
    pub(crate) tool_hint: Option<String>,
}

#[derive(Debug)]
struct Reading {
    window: Window,
    anchor: Anchor,
    newer: VecDeque<HistoryRequest>,
    unseen: bool,
    manual: bool,
}

impl Reading {
    fn restore_at(self) -> Option<(u64, Anchor)> {
        let index = self
            .window
            .records
            .iter()
            .position(|record| record.key == self.anchor.key)?;
        let before = self
            .window
            .records
            .iter()
            .skip(index)
            .take(HISTORY_PAGE / 2)
            .filter_map(|record| record.position)
            .next_back();
        if let Some(before) = before {
            Some((before.saturating_add(1), self.anchor))
        } else {
            let record = self.window.records[..index]
                .iter()
                .rev()
                .find(|record| record.position.is_some())?;
            Some((
                record.position?.saturating_add(1),
                Anchor {
                    key: record.key.clone(),
                    line: AnchorLine::Body(0),
                },
            ))
        }
    }

    fn settle_preview(&mut self, key: &Key, record: Option<&Record>) {
        let Some(index) = self.window.records.iter().position(|old| &old.key == key) else {
            return;
        };
        if let Some(record) = record {
            if self.anchor.key == *key {
                self.anchor.key = record.key.clone();
            }
            self.window.records[index] = record.clone();
        } else {
            self.window.records.remove(index);
            if self.anchor.key == *key {
                if let Some(previous) = self.window.records[..index]
                    .iter()
                    .rev()
                    .find(|old| old.position.is_some())
                {
                    self.anchor = Anchor {
                        key: previous.key.clone(),
                        line: AnchorLine::Body(0),
                    };
                }
            }
        }
    }
}

#[derive(Debug)]
struct Record {
    key: Key,
    turn: Option<u64>,
    turn_number: Option<u64>,
    owner: Option<String>,
    execution_id: Option<u64>,
    position: Option<u64>,
    before_event: Option<u64>,
    body: Body,
    suppress_interruption: bool,
    reasoning_preview: String,
    turn_summary: Option<TurnSummary>,
    rendered: RenderCache,
}

impl Record {
    fn from_entry(entry: HistoryEntry) -> Option<Self> {
        let mut key = Key::Event(entry.id);
        let mut text = capped(entry.text.as_deref(), MAX_BODY_BYTES);
        let reasoning_preview = capped(
            entry
                .details
                .get("reasoning_preview")
                .and_then(Value::as_str),
            MAX_BODY_BYTES,
        );
        let execution_id = entry.details.get("execution_id").and_then(Value::as_u64);
        let body = match entry.kind {
            HistoryKind::User => Body::User {
                sender: entry
                    .details
                    .get("from")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                text,
            },
            HistoryKind::Assistant if text.trim().is_empty() && reasoning_preview.is_empty() => {
                return None
            }
            HistoryKind::Assistant => Body::Assistant(text),
            HistoryKind::ToolCall | HistoryKind::ToolResult => {
                if let Some(id) = entry.details.get("tool_call_id").and_then(Value::as_str) {
                    key = Key::Tool {
                        turn: entry.turn,
                        id: id.to_owned(),
                    };
                }
                Body::Tool(Box::new(Tool::from_entry(&entry)))
            }
            HistoryKind::JobFinished => job_notice(&entry),
            HistoryKind::Question => Body::Notice("Question", text, WARNING),
            HistoryKind::Permission => Body::Notice("Permission", text, WARNING),
            HistoryKind::QuestionReply => Body::Notice("Answer", text, ACCENT),
            HistoryKind::PermissionReply => {
                let decision = match entry.details.get("allow").and_then(Value::as_bool) {
                    Some(true) => "Allowed",
                    Some(false) => "Denied",
                    None => "Recorded",
                };
                Body::Notice("Decision", decision.to_owned(), ACCENT)
            }
            HistoryKind::Status if retry::interruption_execution(&entry).is_some() => {
                if entry.details.get("truncated").and_then(Value::as_bool) == Some(true)
                    && !text.ends_with(DISPLAY_LIMIT_NOTICE)
                {
                    text.push_str(DISPLAY_LIMIT_NOTICE);
                }
                Body::Interrupted(text)
            }
            HistoryKind::Status => {
                if entry.details.get("status").and_then(Value::as_str) == Some("interrupted") {
                    Body::Notice("Execution interrupted", text, WARNING)
                } else {
                    Body::Notice("Execution stopped", text, MUTED)
                }
            }
            HistoryKind::Error => retry::error_body(&entry, text),
            HistoryKind::Retry => retry::retry_body(&entry),
            HistoryKind::TurnSummary if entry.turn_summary.is_some() => Body::TurnSummary,
            HistoryKind::TurnSummary => return None,
            HistoryKind::Other => return None,
        };
        Some(Self {
            key,
            turn: entry.turn,
            turn_number: entry.details.get("turn_number").and_then(Value::as_u64),
            owner: (!entry.agent.is_empty()).then_some(entry.agent),
            execution_id,
            position: Some(entry.id),
            before_event: None,
            body,
            suppress_interruption: false,
            reasoning_preview,
            turn_summary: entry.turn_summary,
            rendered: RenderCache::default(),
        })
    }

    fn merge(&mut self, record: Self) {
        self.turn_number = self.turn_number.or(record.turn_number);
        if self.owner.is_none() {
            self.owner = record.owner;
        }
        self.execution_id = self.execution_id.or(record.execution_id);
        if self.turn_summary.is_none() && record.turn_summary.is_some() {
            self.turn_summary = record.turn_summary;
            self.rendered = RenderCache::default();
        }
        if let (Body::Tool(tool), Body::Tool(incoming)) = (&mut self.body, record.body) {
            tool.merge(*incoming);
            self.position = match (self.position, record.position) {
                (Some(current), Some(incoming)) => Some(current.min(incoming)),
                (None, Some(incoming)) => Some(incoming),
                (current, None) => current,
            };
            self.rendered = RenderCache::default();
        }
    }

    fn reasoning_only(&self) -> bool {
        matches!(&self.body, Body::Assistant(text) if text.trim().is_empty())
            && !self.reasoning_preview.is_empty()
            && self.turn_summary.is_none()
    }

    fn visible(&self) -> bool {
        !self.suppress_interruption
            && (matches!((&self.key,&self.body), (Key::Preview(_), Body::Assistant(text)) if text.is_empty())
                || !matches!(&self.body, Body::Assistant(text) if text.trim().is_empty() && self.reasoning_preview.is_empty()))
    }

    fn tool(&self) -> Option<&Tool> {
        match &self.body {
            Body::Tool(tool) => Some(tool),
            Body::User { .. }
            | Body::Assistant(_)
            | Body::Interrupted(_)
            | Body::Notice(..)
            | Body::ModelError { .. }
            | Body::Retry { .. }
            | Body::TurnSummary => None,
        }
    }

    fn render(
        &mut self,
        layout: Layout<'_>,
        retry_heading: bool,
        retry_end: bool,
    ) -> (&Key, &[Row], Option<DetailRecord<'_>>) {
        let Layout {
            width,
            tools: details,
            assistant,
            agents,
            items,
        } = layout;
        let source_body = match (assistant, &self.body) {
            (AssistantView::Source, Body::Assistant(text)) => Some((text, false)),
            (AssistantView::Source, Body::Interrupted(text)) => Some((text, true)),
            _ => None,
        };
        if let Some((text, interrupted)) = source_body {
            let source = &mut self.rendered.source;
            if source.as_ref().is_none_or(|(cached, _)| *cached != width) {
                let rows = render_assistant_source(text, width, interrupted);
                *source = Some((width, rows));
            }
            return (
                &self.key,
                &source.as_ref().expect("rendered Markdown source").1,
                None,
            );
        }
        let (card, mode, facts) = match &self.body {
            Body::Tool(tool) => {
                let card = tool.agent_card();
                let (mode, facts) = card.as_ref().map_or_else(
                    || (items.tool_mode(&self.key, details), None),
                    |card| agents.display(&self.key, self.position, card.address),
                );
                (card, mode, facts)
            }
            Body::User { .. }
            | Body::Assistant(_)
            | Body::Interrupted(_)
            | Body::Notice(..)
            | Body::ModelError { .. }
            | Body::Retry { .. }
            | Body::TurnSummary => (None, ToolDetails::Expanded, None),
        };
        let cache_key = PreviewLayout {
            width,
            mode,
            in_timeline: details == ToolDetails::GroupMember,
            agent_revision: facts.map_or(0, |facts| facts.paint_revision),
            retry_heading,
            retry_end,
        };
        if self
            .rendered
            .preview
            .as_ref()
            .is_none_or(|(stored, _)| *stored != cache_key)
        {
            let mut lines = match &self.body {
                Body::User { sender, text } => {
                    let title = match sender {
                        Some(sender) => shorten(
                            &format!("From {}", single_line(sender)),
                            width.saturating_sub(usize::from(body_inset(width))),
                        ),
                        None => "You".to_owned(),
                    };
                    render_user(&title, text, width)
                }
                Body::Assistant(text) => {
                    if self.reasoning_only()
                        && !matches!(self.key, Key::Preview(_))
                        && details != ToolDetails::GroupMember
                    {
                        Vec::new()
                    } else {
                        preview::render_assistant(
                            text,
                            !self.reasoning_preview.is_empty(),
                            matches!(self.key, Key::Preview(_)),
                            width,
                        )
                    }
                }
                Body::Interrupted(text) => {
                    message::render_markdown_record("Interrupted", WARNING, text, width)
                }
                Body::Notice(title, text, color) => render_record(
                    title,
                    *color,
                    render_literal(text, Style::default()),
                    width,
                    0,
                ),
                Body::ModelError { .. } | Body::Retry { .. } => {
                    retry::render(&self.body, width, retry_heading, retry_end)
                        .expect("retry history or error body")
                }
                Body::TurnSummary => Vec::new(),
                Body::Tool(tool) => card.as_ref().map_or_else(
                    || tool.render(width, mode),
                    |card| card.render(width, mode, facts, cache_key.in_timeline),
                ),
            };
            if let Some(summary) = &self.turn_summary {
                lines.push(turn_summary::inline(summary, self.turn_number, width));
            }
            self.rendered.preview = Some((cache_key, lines));
        }
        (
            &self.key,
            &self.rendered.preview.as_ref().expect("rendered record").1,
            card.map(DetailRecord::Agent)
                .or_else(|| self.tool().map(DetailRecord::Tool))
                .or_else(|| {
                    (!self.reasoning_preview.is_empty())
                        .then_some(DetailRecord::Reasoning(&self.reasoning_preview))
                }),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewLayout {
    width: usize,
    mode: ToolDetails,
    in_timeline: bool,
    agent_revision: u64,
    retry_heading: bool,
    retry_end: bool,
}

fn body_inset(width: usize) -> u16 {
    if width >= MIN_TRANSCRIPT_INDENT_WIDTH {
        TRANSCRIPT_INDENT
    } else {
        0
    }
}

fn render_rows(
    lines: Vec<Line<'static>>,
    width: usize,
    limit: usize,
    inset: u16,
) -> impl Iterator<Item = Row> {
    wrap_bounded(lines, width.saturating_sub(usize::from(inset)), limit)
        .into_iter()
        .map(move |mut line| {
            if inset > 0 && !line.spans.is_empty() {
                line.spans
                    .insert(0, Span::raw(" ".repeat(usize::from(inset))));
            }
            Row {
                line,
                inset,
                links: Vec::new(),
            }
        })
}

fn capped(text: Option<&str>, bytes: usize) -> String {
    let text = text.unwrap_or_default();
    let cut = cap(text, bytes);
    let mut result = cut.to_owned();
    if cut.len() < text.len() {
        result.push_str(DISPLAY_LIMIT_NOTICE);
    }
    result
}

fn append_bounded(out: &mut String, text: &str, limit: usize) -> bool {
    let remaining = limit.saturating_sub(out.len());
    if remaining == 0 || text.is_empty() {
        return false;
    }
    let prefix = cap(text, remaining);
    out.push_str(prefix);
    if prefix.len() < text.len() {
        out.push_str(DISPLAY_LIMIT_NOTICE);
    }
    true
}

#[cfg(test)]
mod retry_history_tests;
#[cfg(test)]
mod tests;
