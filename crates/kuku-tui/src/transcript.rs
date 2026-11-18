use std::collections::VecDeque;

use kuku_client::{HistoryEntry, HistoryKind, HistoryPage};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::keys::Shortcut;
use crate::markup::{render_literal, render_markdown, wrap_bounded};
use crate::text::{cap, shorten, single_line};
use crate::theme::{
    emphasis, ACCENT, ERROR, MIN_TRANSCRIPT_INDENT_WIDTH, MUTED, TRANSCRIPT_INDENT, WARNING,
};

mod agent;
mod message;
mod tool;

pub(crate) use self::agent::AgentCardKey;
use self::agent::{AgentCard, AgentCards, VisibleAgent};
use self::message::{render_record, render_user};
use self::tool::Tool;

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
struct Anchor {
    key: Key,
    line: AnchorLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorLine {
    Turn,
    Body(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryRequest {
    Older(u64),
    Newer { first: u64, before: u64 },
    Restore(u64),
}

impl HistoryRequest {
    pub(crate) fn before(self) -> u64 {
        match self {
            Self::Older(before) | Self::Newer { before, .. } | Self::Restore(before) => before,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ToolDetails {
    #[default]
    Expanded,
    Collapsed,
}

#[derive(Debug, Default)]
pub(crate) struct Transcript {
    recent: Window,
    reading: Option<Reading>,
    restore: Option<(u64, Anchor)>,
    details: ToolDetails,
    current_execution: Option<u64>,
    agent_cards: AgentCards,
}

impl Transcript {
    pub(crate) fn replace_recent(&mut self, page: HistoryPage) {
        self.recent = Window::from_page(page);
        self.current_execution = None;
        self.agent_cards.visible.clear();
    }

    pub(crate) fn suspend(&mut self) {
        if let Some(reading) = self.reading.take() {
            self.restore = reading.restore_at();
        }
        self.recent = Window::default();
        self.current_execution = None;
        self.agent_cards.visible.clear();
    }

    pub(crate) fn restore_request(&self) -> Option<HistoryRequest> {
        self.restore
            .as_ref()
            .map(|(before, _)| HistoryRequest::Restore(*before))
    }

    pub(crate) fn apply(&mut self, entry: HistoryEntry, execution: Option<u64>) {
        let assistant = entry.kind == HistoryKind::Assistant;
        let record = Record::from_entry(entry);
        if assistant {
            if let Some(execution) = execution.or(self.current_execution) {
                let key = Key::Preview(execution);
                self.recent.remove(&key);
                if let Some(reading) = &mut self.reading {
                    reading.settle_preview(&key, record.as_ref());
                }
            }
        }
        let Some(record) = record else { return };
        if let Some(reading) = &mut self.reading {
            if matches!(record.key, Key::Tool { .. }) && reading.window.contains(&record.key) {
                reading.window.insert(record.clone());
            }
        }
        self.recent.insert(record);
        self.recent.trim_front();
    }

    pub(crate) fn live(
        &mut self,
        execution: u64,
        turn: Option<u64>,
        turn_number: Option<u64>,
        event: &Value,
    ) {
        if self.current_execution != Some(execution) {
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
                            record.rendered = None;
                        }
                    }
                }
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
                }
            }
            Some("done" | "cancelled" | "error") => self.clear_preview(),
            Some(_) | None => {}
        }
        if self.recent.records.len() > count {
            self.recent.limit_live();
        }
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
        self.reading = None;
        self.restore = None;
    }

    pub(crate) fn toggle_details(&mut self) -> bool {
        let expanded = self.details_can_expand();
        self.details = if expanded {
            ToolDetails::Expanded
        } else {
            ToolDetails::Collapsed
        };
        self.agent_cards.set_details(expanded);
        expanded
    }

    pub(crate) fn last_reply(&self) -> Option<&str> {
        self.recent
            .records
            .iter()
            .rev()
            .find_map(|record| match &record.body {
                Body::Assistant(text) if record.position.is_some() => Some(text.as_str()),
                _ => None,
            })
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
                Body::Tool(tool) => Some(tool.view()),
                Body::User { .. } | Body::Assistant(_) | Body::Notice(..) => None,
            })
            .collect()
    }

    pub(crate) fn scroll(
        &mut self,
        amount: isize,
        width: usize,
        height: usize,
    ) -> Option<HistoryRequest> {
        self.link_recent_agent_cards();
        if self.reading.is_none() {
            if !amount.is_negative() || self.recent.records.is_empty() {
                return None;
            }
            let mut window = self.recent.clone();
            let rows = window.row_count(width, self.details, &self.agent_cards);
            let anchor = window.anchor_at(
                rows.saturating_sub(height),
                width,
                self.details,
                &self.agent_cards,
            )?;
            self.reading = Some(Reading {
                window,
                anchor,
                newer: VecDeque::new(),
            });
        }
        let reading = self.reading.as_mut().expect("read window");
        let rows = reading
            .window
            .row_count(width, self.details, &self.agent_cards);
        let start =
            reading
                .window
                .position_of(&reading.anchor, width, self.details, &self.agent_cards);
        let bottom = rows.saturating_sub(height);
        let next = start.saturating_add_signed(amount).min(bottom);
        if let Some(anchor) = reading
            .window
            .anchor_at(next, width, self.details, &self.agent_cards)
        {
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
            | HistoryRequest::Restore(_) => {}
        }
    }

    pub(crate) fn project(&mut self, width: usize, height: usize) -> TranscriptView {
        self.link_recent_agent_cards();
        let (window, start, following) = if let Some(reading) = &mut self.reading {
            let start =
                reading
                    .window
                    .position_of(&reading.anchor, width, self.details, &self.agent_cards);
            (&mut reading.window, start, false)
        } else {
            let rows = self
                .recent
                .row_count(width, self.details, &self.agent_cards);
            (&mut self.recent, rows.saturating_sub(height), true)
        };
        let mut lines = Vec::with_capacity(height);
        let mut row_insets = Vec::with_capacity(height);
        let mut tool_visible = false;
        let mut visible_agents = Vec::new();
        let mut skip = start;
        for block in window.blocks(width, self.details, &self.agent_cards) {
            if lines.len() == height {
                break;
            }
            if skip >= block.height() {
                skip -= block.height();
                continue;
            }
            if let Some(turn) = block.heading {
                if skip == 0 {
                    lines.push(render_turn(turn, width));
                    row_insets.push(0);
                }
                skip = skip.saturating_sub(1);
            }
            let first = lines.len();
            for row in block.rows.iter().skip(skip).take(height - lines.len()) {
                lines.push(row.line.clone());
                row_insets.push(row.inset);
                tool_visible |= block.tool && !row.line.spans.is_empty();
            }
            if let Some(agent) = block.agent {
                let last = lines.len()
                    - usize::from(
                        lines.len() > first
                            && lines.last().is_some_and(|line| line.spans.is_empty()),
                    );
                if last > first {
                    visible_agents.push(VisibleAgent {
                        rows: first..last,
                        key: block.key.clone(),
                        address: agent.address.to_owned(),
                        summary: agent.choice_summary(),
                    });
                }
            }
            skip = 0;
        }
        self.agent_cards.visible = visible_agents;
        let agent_visible = !self.agent_cards.visible.is_empty();
        let tool_hint = (tool_visible || agent_visible).then(|| {
            let action = if self.details_can_expand() {
                "show"
            } else {
                "hide"
            };
            let menu = if agent_visible { " · /details" } else { "" };
            format!("{} {action}{menu}", Shortcut::DETAILS.label)
        });
        TranscriptView {
            lines,
            row_insets,
            following,
            tool_hint,
        }
    }

    fn details_can_expand(&self) -> bool {
        self.details == ToolDetails::Collapsed || self.agent_cards.has_collapsed_visible()
    }
}

pub(crate) struct TranscriptView {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) row_insets: Vec<u16>,
    pub(crate) following: bool,
    pub(crate) tool_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolView {
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) invocation: String,
    pub(crate) details: String,
    pub(crate) diff: String,
    pub(crate) output_id: Option<String>,
}

#[derive(Debug)]
struct Reading {
    window: Window,
    anchor: Anchor,
    newer: VecDeque<HistoryRequest>,
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

#[derive(Debug, Clone, Default)]
struct Window {
    records: Vec<Record>,
    older: Option<u64>,
}

impl Window {
    fn from_page(page: HistoryPage) -> Self {
        let mut window = Self {
            older: page.next_before,
            ..Self::default()
        };
        for record in page.entries.into_iter().filter_map(Record::from_entry) {
            window.insert(record);
        }
        window
    }

    fn contains(&self, key: &Key) -> bool {
        self.records.iter().any(|record| &record.key == key)
    }

    fn remove(&mut self, key: &Key) {
        self.records.retain(|record| &record.key != key);
    }

    fn insert(&mut self, record: Record) {
        if let Some(previous) = self.records.iter_mut().find(|old| old.key == record.key) {
            previous.merge(record);
        } else {
            self.records.push(record);
        }
        self.records
            .sort_by_key(|record| record.position.unwrap_or(u64::MAX));
    }

    fn clear_preview(&mut self) {
        self.records.retain(|record| record.position.is_some());
        for record in &mut self.records {
            if let Body::Tool(tool) = &mut record.body {
                if tool.clear_live() {
                    record.rendered = None;
                }
            }
        }
    }

    fn trim_front(&mut self) {
        if self.records.len() > MAX_RECORDS {
            self.records.drain(..self.records.len() - MAX_RECORDS);
            self.older = self.records.iter().find_map(|record| record.position);
        }
    }

    fn trim_back(&mut self) -> Option<HistoryRequest> {
        if self.records.len() <= MAX_RECORDS {
            return None;
        }
        let truncated = &self.records[MAX_RECORDS..];
        let first = truncated.iter().find_map(|record| record.position);
        let last = truncated.iter().rev().find_map(|record| record.position);
        self.records.truncate(MAX_RECORDS);
        Some(HistoryRequest::Newer {
            first: first?,
            before: last?.saturating_add(1),
        })
    }

    fn limit_live(&mut self) {
        let mut count = 0;
        self.records.retain(|record| {
            if record.position.is_some() {
                return true;
            }
            count += 1;
            count <= MAX_LIVE_RECORDS
        });
    }

    fn record_or_insert(
        &mut self,
        key: Key,
        turn: Option<u64>,
        turn_number: Option<u64>,
        body: impl FnOnce() -> Body,
    ) -> &mut Record {
        if let Some(index) = self.records.iter().position(|record| record.key == key) {
            let record = &mut self.records[index];
            record.turn = record.turn.or(turn);
            record.turn_number = record.turn_number.or(turn_number);
            return record;
        }
        self.records.push(Record {
            key,
            turn,
            turn_number,
            position: None,
            body: body(),
            rendered: None,
        });
        self.records.last_mut().expect("inserted record")
    }

    fn tool_live(&mut self, key: Key, turn: Option<u64>, turn_number: Option<u64>, event: &Value) {
        let record = self.record_or_insert(key, turn, turn_number, || Body::Tool(Box::default()));
        if let Body::Tool(tool) = &mut record.body {
            if tool.apply_live(event) {
                record.rendered = None;
            }
        }
    }

    fn row_count(&mut self, width: usize, details: ToolDetails, agents: &AgentCards) -> usize {
        self.blocks(width, details, agents)
            .map(|block| block.height())
            .sum()
    }

    fn position_of(
        &mut self,
        anchor: &Anchor,
        width: usize,
        details: ToolDetails,
        agents: &AgentCards,
    ) -> usize {
        let mut start = 0;
        for block in self.blocks(width, details, agents) {
            if block.key == &anchor.key {
                return start + block.offset(anchor.line);
            }
            start += block.height();
        }
        0
    }

    fn anchor_at(
        &mut self,
        mut row: usize,
        width: usize,
        details: ToolDetails,
        agents: &AgentCards,
    ) -> Option<Anchor> {
        for block in self.blocks(width, details, agents) {
            if row < block.height() {
                return Some(Anchor {
                    key: block.key.clone(),
                    line: match (block.heading, row) {
                        (Some(_), 0) => AnchorLine::Turn,
                        (Some(_), row) => AnchorLine::Body(row - 1),
                        (None, row) => AnchorLine::Body(row),
                    },
                });
            }
            row -= block.height();
        }
        None
    }

    fn blocks<'a>(
        &'a mut self,
        width: usize,
        details: ToolDetails,
        agents: &'a AgentCards,
    ) -> impl Iterator<Item = Block<'a>> {
        let mut records = self
            .records
            .iter_mut()
            .filter(|record| record.visible())
            .peekable();
        let from_start = self.older.is_none();
        let mut previous_turn = None;
        std::iter::from_fn(move || {
            let record = records.next()?;
            let known_number = record.turn_number.filter(|&number| number > 0);
            let heading = record
                .turn
                .filter(|&turn| {
                    Some(turn) != previous_turn && (previous_turn.is_some() || from_start)
                })
                .map(|_| known_number.map_or(TurnHeading::Unnumbered, TurnHeading::Numbered));
            previous_turn = record.turn.or(previous_turn);
            let tool = matches!(record.body, Body::Tool(_));
            let adjacent_tool = tool
                && records.peek().is_some_and(|next| {
                    next.turn == record.turn && matches!(next.body, Body::Tool(_))
                });
            let (key, rows, agent) = record.render(width, details, agents);
            let rows = if adjacent_tool {
                &rows[..rows.len() - 1]
            } else {
                rows
            };
            Some(Block {
                key,
                heading,
                rows,
                tool: tool && agent.is_none(),
                agent,
            })
        })
    }
}

struct Block<'a> {
    key: &'a Key,
    heading: Option<TurnHeading>,
    rows: &'a [Row],
    tool: bool,
    agent: Option<AgentCard<'a>>,
}

#[derive(Debug, Clone, Copy)]
enum TurnHeading {
    Numbered(u64),
    Unnumbered,
}

impl Block<'_> {
    fn height(&self) -> usize {
        self.rows.len() + usize::from(self.heading.is_some())
    }

    fn offset(&self, line: AnchorLine) -> usize {
        match line {
            AnchorLine::Turn => 0,
            AnchorLine::Body(line) => {
                line.min(self.rows.len().saturating_sub(1)) + usize::from(self.heading.is_some())
            }
        }
    }
}

#[derive(Debug)]
struct Record {
    key: Key,
    turn: Option<u64>,
    turn_number: Option<u64>,
    position: Option<u64>,
    body: Body,
    rendered: Option<((usize, ToolDetails, u64), Vec<Row>)>,
}

impl Clone for Record {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            turn: self.turn,
            turn_number: self.turn_number,
            position: self.position,
            body: self.body.clone(),
            rendered: None,
        }
    }
}

impl Record {
    fn from_entry(entry: HistoryEntry) -> Option<Self> {
        let mut key = Key::Event(entry.id);
        let text = capped(entry.text.as_deref(), MAX_BODY_BYTES);
        let body = match entry.kind {
            HistoryKind::User => Body::User {
                sender: entry
                    .details
                    .get("from")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                text,
            },
            HistoryKind::Assistant if text.trim().is_empty() => return None,
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
            HistoryKind::Status => {
                if entry.details.get("status").and_then(Value::as_str) == Some("interrupted") {
                    Body::Notice("Execution interrupted", text, WARNING)
                } else {
                    Body::Notice("Execution stopped", text, MUTED)
                }
            }
            HistoryKind::Error => Body::Notice("Provider error", text, ERROR),
            HistoryKind::Other => return None,
        };
        Some(Self {
            key,
            turn: entry.turn,
            turn_number: entry.details.get("turn_number").and_then(Value::as_u64),
            position: Some(entry.id),
            body,
            rendered: None,
        })
    }

    fn merge(&mut self, record: Self) {
        self.turn_number = self.turn_number.or(record.turn_number);
        if let (Body::Tool(tool), Body::Tool(incoming)) = (&mut self.body, record.body) {
            tool.merge(*incoming);
            self.position = match (self.position, record.position) {
                (Some(current), Some(incoming)) => Some(current.min(incoming)),
                (None, Some(incoming)) => Some(incoming),
                (current, None) => current,
            };
            self.rendered = None;
        }
    }

    fn visible(&self) -> bool {
        !matches!(&self.body, Body::Assistant(text) if text.trim().is_empty())
    }

    fn render(
        &mut self,
        width: usize,
        details: ToolDetails,
        agents: &AgentCards,
    ) -> (&Key, &[Row], Option<AgentCard<'_>>) {
        let (card, mode, facts) = match &self.body {
            Body::Tool(tool) => {
                let card = tool.agent_card();
                let (mode, facts) = card.as_ref().map_or((details, None), |card| {
                    agents.display(&self.key, self.position, card.address)
                });
                (card, mode, facts)
            }
            Body::User { .. } | Body::Assistant(_) | Body::Notice(..) => {
                (None, ToolDetails::Expanded, None)
            }
        };
        let layout = (width, mode, facts.map_or(0, |facts| facts.paint_revision));
        if self
            .rendered
            .as_ref()
            .is_none_or(|(stored, _)| *stored != layout)
        {
            let lines = match &self.body {
                Body::User { sender, text } => {
                    let title = match sender {
                        Some(sender) => shorten(
                            &format!("From {}", single_line(sender)),
                            width.saturating_sub(usize::from(body_inset(width))),
                        ),
                        None if width >= 10 => "You (sent)".to_owned(),
                        None => "You".to_owned(),
                    };
                    render_user(&title, text, width)
                }
                Body::Assistant(text) => render_record(
                    if width >= 9 { "Assistant" } else { "AI" },
                    MUTED,
                    render_markdown(text, width, MAX_VISUAL_ROWS),
                    width,
                    0,
                ),
                Body::Notice(title, text, color) => render_record(
                    title,
                    *color,
                    render_literal(text, Style::default()),
                    width,
                    0,
                ),
                Body::Tool(tool) => card.as_ref().map_or_else(
                    || tool.render(width, mode),
                    |card| card.render(width, mode, facts),
                ),
            };
            self.rendered = Some((layout, lines));
        }
        (
            &self.key,
            &self.rendered.as_ref().expect("rendered record").1,
            card,
        )
    }
}

#[derive(Debug, Clone)]
enum Body {
    User {
        sender: Option<String>,
        text: String,
    },
    Assistant(String),
    Notice(&'static str, String, Color),
    Tool(Box<Tool>),
}

#[derive(Debug)]
struct Row {
    line: Line<'static>,
    inset: u16,
}

impl Row {
    fn plain(line: Line<'static>) -> Self {
        Self { line, inset: 0 }
    }
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
            Row { line, inset }
        })
}

fn render_turn(turn: TurnHeading, width: usize) -> Line<'static> {
    let label = match turn {
        TurnHeading::Numbered(number) if width < 6 => format!("T{number}"),
        TurnHeading::Numbered(number) => format!("Turn {number}"),
        TurnHeading::Unnumbered => "Turn".to_owned(),
    };
    Line::styled(shorten(&label, width), emphasis(MUTED))
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
mod tests;
