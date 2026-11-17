use std::collections::VecDeque;

use kuku_client::{HistoryEntry, HistoryKind, HistoryPage};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::markup::{render_diff, render_literal, render_markdown, wrap_bounded, ACCENT, MUTED};
use crate::text::{cap, single_line};

pub(crate) const HISTORY_PAGE: usize = 48;
const MAX_RECORDS: usize = HISTORY_PAGE * 3;
const MAX_LIVE_RECORDS: usize = 32;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_DETAIL_BYTES: usize = 12 * 1024;
const MAX_BACKTRACK_PAGES: usize = 128;
const MAX_VISUAL_ROWS: usize = 768;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Event(u64),
    Preview(u64),
    Tool { turn: Option<u64>, id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Anchor {
    key: Key,
    line: usize,
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

#[derive(Debug, Default)]
pub(crate) struct Transcript {
    recent: Window,
    reading: Option<Reading>,
    restore: Option<(u64, Anchor)>,
    expanded: bool,
    current_execution: Option<u64>,
}

impl Transcript {
    pub(crate) fn replace_recent(&mut self, page: HistoryPage) {
        self.recent = Window::from_page(page);
        self.current_execution = None;
    }

    pub(crate) fn suspend(&mut self) {
        if let Some(reading) = self.reading.take() {
            self.restore = reading.restore_at();
        }
        self.recent = Window::default();
        self.current_execution = None;
    }

    pub(crate) fn restore_request(&self) -> Option<HistoryRequest> {
        self.restore
            .as_ref()
            .map(|(before, _)| HistoryRequest::Restore(*before))
    }

    pub(crate) fn apply(&mut self, entry: HistoryEntry, execution: Option<u64>) {
        let Some(record) = Record::from_entry(entry) else {
            return;
        };
        if matches!(record.body, Body::Assistant(_)) {
            if let Some(execution) = execution.or(self.current_execution) {
                let key = Key::Preview(execution);
                self.recent.remove(&key);
                if let Some(reading) = &mut self.reading {
                    reading.replace(&key, record.clone());
                }
            }
        }
        if let Some(reading) = &mut self.reading {
            if matches!(record.key, Key::Tool { .. }) && reading.window.contains(&record.key) {
                reading.window.insert(record.clone());
            }
        }
        self.recent.insert(record);
        self.recent.trim_front();
    }

    pub(crate) fn live(&mut self, execution: u64, turn: Option<u64>, event: &Value) {
        if self.current_execution != Some(execution) {
            self.clear_preview();
            self.current_execution = Some(execution);
        }
        match event["type"].as_str() {
            Some("text") => {
                if let Some(text) = event["content"].as_str() {
                    let key = Key::Preview(execution);
                    let record = self
                        .recent
                        .record_or_insert(key, || Body::Assistant(String::new()));
                    if let Body::Assistant(body) = &mut record.body {
                        append_bounded(body, text, MAX_BODY_BYTES);
                        record.rendered = None;
                    }
                }
            }
            Some("tool_start" | "tool_output" | "tool_end") => self.recent.tool_live(turn, event),
            Some("done" | "cancelled" | "error") => self.clear_preview(),
            Some(_) | None => {}
        }
        self.recent.limit_live();
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
                    reading.anchor = Anchor { key, line: 0 };
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
        self.expanded = !self.expanded;
        self.expanded
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
            .filter_map(|record| {
                if let Body::Tool(tool) = &record.body {
                    Some(ToolView {
                        name: tool.name.clone(),
                        summary: tool.summary.clone(),
                        details: tool.details.clone(),
                        diff: tool.display_diff().unwrap_or_default().to_owned(),
                        output_id: tool.output_id.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    pub(crate) fn scroll(
        &mut self,
        amount: isize,
        width: usize,
        height: usize,
    ) -> Option<HistoryRequest> {
        if self.reading.is_none() {
            if !amount.is_negative() || self.recent.records.is_empty() {
                return None;
            }
            let mut window = self.recent.clone();
            let rows = window.row_count(width, self.expanded);
            let anchor = window.anchor_at(rows.saturating_sub(height), width, self.expanded)?;
            self.reading = Some(Reading {
                window,
                anchor,
                newer: VecDeque::new(),
            });
        }
        let reading = self.reading.as_mut().expect("read window");
        let rows = reading.window.row_count(width, self.expanded);
        let start = reading
            .window
            .position_of(&reading.anchor, width, self.expanded);
        let bottom = rows.saturating_sub(height);
        let next = start.saturating_add_signed(amount).min(bottom);
        if let Some(anchor) = reading.window.anchor_at(next, width, self.expanded) {
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
        let (window, start, following) = if let Some(reading) = &mut self.reading {
            let start = reading
                .window
                .position_of(&reading.anchor, width, self.expanded);
            (&mut reading.window, start, false)
        } else {
            let rows = self.recent.row_count(width, self.expanded);
            (&mut self.recent, rows.saturating_sub(height), true)
        };
        let mut lines = Vec::with_capacity(height);
        let mut skip = start;
        for record in &mut window.records {
            let block = record.lines(width, self.expanded);
            if skip >= block.len() {
                skip -= block.len();
                continue;
            }
            lines.extend(block.iter().skip(skip).take(height - lines.len()).cloned());
            if lines.len() == height {
                break;
            }
            skip = 0;
        }
        TranscriptView { lines, following }
    }
}

pub(crate) struct TranscriptView {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) following: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolView {
    pub(crate) name: String,
    pub(crate) summary: String,
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
                    line: 0,
                },
            ))
        }
    }

    fn replace(&mut self, key: &Key, record: Record) {
        if let Some(preview) = self.window.records.iter_mut().find(|old| &old.key == key) {
            if &self.anchor.key == key {
                self.anchor.key = record.key.clone();
            }
            *preview = record;
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
                if !tool.progress.is_empty() {
                    tool.progress.clear();
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

    fn record_or_insert(&mut self, key: Key, body: impl FnOnce() -> Body) -> &mut Record {
        if let Some(index) = self.records.iter().position(|record| record.key == key) {
            return &mut self.records[index];
        }
        self.records.push(Record {
            key,
            position: None,
            body: body(),
            rendered: None,
        });
        self.records.last_mut().expect("inserted record")
    }

    fn tool_live(&mut self, turn: Option<u64>, event: &Value) {
        let Some(id) = event["id"].as_str() else {
            return;
        };
        let key = Key::Tool {
            turn,
            id: id.to_owned(),
        };
        let record = self.record_or_insert(key, || Body::Tool(Box::default()));
        let Body::Tool(tool) = &mut record.body else {
            return;
        };
        if tool.complete {
            return;
        }
        match event["type"].as_str() {
            Some("tool_start") => {
                tool.name = capped(event["tool"].as_str(), MAX_DETAIL_BYTES);
                tool.summary = capped(event["summary"].as_str(), MAX_DETAIL_BYTES);
            }
            Some("tool_output") => {
                let event = &event["event"];
                if let Some(text) = event["stdout"]
                    .as_str()
                    .or_else(|| event["stderr"].as_str())
                {
                    append_bounded(&mut tool.progress, text, MAX_DETAIL_BYTES);
                }
            }
            Some("tool_end") => {
                tool.summary = capped(event["summary"].as_str(), MAX_DETAIL_BYTES);
                tool.status = capped(event["status"].as_str(), MAX_DETAIL_BYTES);
            }
            Some(_) | None => {}
        }
        record.rendered = None;
    }

    fn row_count(&mut self, width: usize, expanded: bool) -> usize {
        self.records
            .iter_mut()
            .map(|record| record.lines(width, expanded).len())
            .sum()
    }

    fn position_of(&mut self, anchor: &Anchor, width: usize, expanded: bool) -> usize {
        let mut start = 0;
        for record in &mut self.records {
            let height = record.lines(width, expanded).len();
            if record.key == anchor.key {
                return start + anchor.line.min(height.saturating_sub(1));
            }
            start += height;
        }
        0
    }

    fn anchor_at(&mut self, mut row: usize, width: usize, expanded: bool) -> Option<Anchor> {
        for record in &mut self.records {
            let height = record.lines(width, expanded).len();
            if row < height {
                return Some(Anchor {
                    key: record.key.clone(),
                    line: row,
                });
            }
            row -= height;
        }
        self.records.last().map(|record| Anchor {
            key: record.key.clone(),
            line: 0,
        })
    }
}

#[derive(Debug)]
struct Record {
    key: Key,
    position: Option<u64>,
    body: Body,
    rendered: Option<(usize, bool, Vec<Line<'static>>)>,
}

impl Clone for Record {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
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
            HistoryKind::User => Body::User(text),
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
            HistoryKind::Question => Body::Notice("Question", text, Color::Yellow),
            HistoryKind::Permission => Body::Notice("Permission", text, Color::Yellow),
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
                    Body::Notice("Execution interrupted", text, Color::Yellow)
                } else {
                    Body::Notice("Execution stopped", text, MUTED)
                }
            }
            HistoryKind::Error => Body::Notice("Provider error", text, Color::Red),
            HistoryKind::Other => return None,
        };
        Some(Self {
            key,
            position: Some(entry.id),
            body,
            rendered: None,
        })
    }

    fn merge(&mut self, record: Self) {
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

    fn lines(&mut self, width: usize, expanded: bool) -> &[Line<'static>] {
        if self
            .rendered
            .as_ref()
            .is_none_or(|(stored, details, _)| *stored != width || *details != expanded)
        {
            let lines = match &self.body {
                Body::User(text) => {
                    render_record("You", ACCENT, render_literal(text, Style::default()))
                }
                Body::Assistant(text) => {
                    render_record("Assistant", Color::Green, render_markdown(text))
                }
                Body::Notice(title, text, color) => {
                    render_record(title, *color, render_literal(text, Style::default()))
                }
                Body::Tool(tool) => tool.render(expanded),
            };
            self.rendered = Some((width, expanded, wrap_bounded(lines, width, MAX_VISUAL_ROWS)));
        }
        &self.rendered.as_ref().expect("rendered record").2
    }
}

#[derive(Debug, Clone)]
enum Body {
    User(String),
    Assistant(String),
    Notice(&'static str, String, Color),
    Tool(Box<Tool>),
}

#[derive(Debug, Clone, Default)]
struct Tool {
    name: String,
    summary: String,
    status: String,
    details: String,
    diff: String,
    requested_change: String,
    progress: String,
    exit_code: Option<i64>,
    output_id: Option<String>,
    complete: bool,
}

impl Tool {
    fn from_entry(entry: &HistoryEntry) -> Self {
        let get = |field: &str| entry.details.get(field).and_then(Value::as_str);
        let structured = entry.details.get("structured").unwrap_or(&Value::Null);
        let args = entry.details.get("args").map(ToString::to_string);
        let requested_change = if get("tool") == Some("edit_file") {
            entry
                .details
                .get("args")
                .and_then(edit_preview)
                .unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            name: capped(get("tool"), MAX_DETAIL_BYTES),
            summary: capped(get("summary").or(entry.text.as_deref()), MAX_DETAIL_BYTES),
            status: capped(get("status"), MAX_DETAIL_BYTES),
            details: capped(get("model_content").or(args.as_deref()), MAX_DETAIL_BYTES),
            diff: capped(
                structured["diff"].as_str().or_else(|| get("diff")),
                MAX_DETAIL_BYTES,
            ),
            requested_change,
            exit_code: structured["exit_code"].as_i64(),
            output_id: get("output_id").map(str::to_owned),
            complete: entry.kind == HistoryKind::ToolResult,
            ..Self::default()
        }
    }

    fn merge(&mut self, incoming: Self) {
        if !incoming.name.is_empty() {
            self.name = incoming.name;
        }
        if !incoming.requested_change.is_empty() {
            self.requested_change = incoming.requested_change;
        }
        if self.complete && !incoming.complete {
            return;
        }
        if !incoming.summary.is_empty() {
            self.summary = incoming.summary;
        }
        if !incoming.details.is_empty() {
            self.details = incoming.details;
        }
        if incoming.complete {
            self.status = incoming.status;
            self.diff = incoming.diff;
            self.exit_code = incoming.exit_code;
            self.output_id = incoming.output_id;
            self.progress.clear();
            self.complete = true;
        }
    }

    fn render(&self, expanded: bool) -> Vec<Line<'static>> {
        let name = if self.name.is_empty() {
            "Tool"
        } else {
            &self.name
        };
        let status = if self.status.is_empty() {
            "running"
        } else {
            &self.status
        };
        let icon = if expanded { '▾' } else { '▸' };
        let exit = self
            .exit_code
            .map_or_else(String::new, |exit| format!(" · exit {exit}"));
        let mut lines = vec![Line::from(vec![
            Span::styled(
                format!("{icon} {}", single_line(name)),
                Style::default().fg(ACCENT),
            ),
            Span::styled(
                format!(" · {}{exit}", single_line(status)),
                Style::default().fg(MUTED),
            ),
        ])];
        if !self.summary.is_empty() {
            lines.extend(render_literal(&self.summary, Style::default()));
        }
        if expanded {
            lines.extend(self.render_details());
        }
        lines.push(Line::default());
        lines
    }

    fn render_details(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(diff) = self.display_diff() {
            lines.extend(render_diff(diff));
        } else if !self.details.is_empty() && self.details != self.summary {
            lines.extend(render_literal(
                &self.details,
                Style::default().fg(Color::Gray),
            ));
        }
        if !self.progress.is_empty() {
            lines.extend(render_literal(
                &self.progress,
                Style::default().fg(Color::Gray),
            ));
        }
        if self.output_id.is_some() {
            lines.push(Line::styled(
                "Saved output · /tools",
                Style::default().fg(ACCENT),
            ));
        }
        lines
    }

    fn display_diff(&self) -> Option<&str> {
        if !self.diff.is_empty() {
            Some(&self.diff)
        } else if self.complete
            && self.status.eq_ignore_ascii_case("ok")
            && !self.requested_change.is_empty()
        {
            Some(&self.requested_change)
        } else {
            None
        }
    }
}

fn edit_preview(args: &Value) -> Option<String> {
    let old = cap(args["old_text"].as_str()?, MAX_DETAIL_BYTES / 2);
    let new = cap(args["new_text"].as_str()?, MAX_DETAIL_BYTES / 2);
    let mut preview = String::from("Replacement\n");
    for (prefix, text) in [('-', old), ('+', new)] {
        for line in text.lines() {
            preview.push(prefix);
            preview.push_str(line);
            preview.push('\n');
        }
    }
    Some(preview)
}

fn render_record(title: &str, color: Color, mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    lines.insert(0, Line::styled(title.to_owned(), style));
    if lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::default());
    }
    lines
}

fn capped(text: Option<&str>, bytes: usize) -> String {
    let text = text.unwrap_or_default();
    let cut = cap(text, bytes);
    let mut result = cut.to_owned();
    if cut.len() < text.len() {
        result.push_str("\n… Display limit reached");
    }
    result
}

fn append_bounded(out: &mut String, text: &str, limit: usize) {
    if out.len() < limit {
        out.push_str(&capped(Some(text), limit - out.len()));
    }
}

#[cfg(test)]
mod tests {
    use kuku_client::{HistoryEntry, HistoryKind, HistoryPage};
    use serde_json::json;

    use super::{HistoryRequest, Transcript, HISTORY_PAGE};

    fn response(id: u64, body: String) -> HistoryEntry {
        HistoryEntry {
            id,
            agent: "main".into(),
            turn: Some(1),
            kind: HistoryKind::Assistant,
            text: Some(body),
            details: Default::default(),
        }
    }

    fn page(first: u64, before: u64) -> HistoryPage {
        HistoryPage {
            entries: (first..before)
                .map(|id| response(id, format!("Answer {id}")))
                .collect(),
            next_before: (first > 1).then_some(first),
        }
    }

    fn rows(transcript: &mut Transcript) -> Vec<String> {
        transcript
            .project(44, 10)
            .lines
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn frozen_history_survives_older_page_arrival_and_many_recent_commits() {
        let mut transcript = Transcript::default();
        transcript.replace_recent(page(51, 70));
        let older = transcript.scroll(-200, 44, 10);
        assert_eq!(Some(HistoryRequest::Older(51)), older);
        let anchored = rows(&mut transcript);
        transcript.apply_page(older.unwrap(), page(3, 51));
        for id in 70..240 {
            transcript.apply(response(id, format!("Answer {id}")), Some(100));
        }
        assert_eq!(anchored, rows(&mut transcript));
        transcript.show_latest();
        assert!(rows(&mut transcript)
            .iter()
            .any(|row| row.contains("Answer 239")));
    }

    #[test]
    fn evicted_view_restores_its_saved_anchor_from_durable_history() {
        let mut transcript = Transcript::default();
        transcript.replace_recent(page(51, 91));
        transcript.scroll(-36, 44, 10);
        let anchored = rows(&mut transcript);
        transcript.suspend();
        let request = transcript.restore_request().unwrap();
        let before = request.before();
        transcript.replace_recent(page(101, 141));
        transcript.apply_page(request, page(before - HISTORY_PAGE as u64, before));
        assert_eq!(anchored, rows(&mut transcript));
    }

    #[test]
    fn durable_response_replaces_its_live_preview() {
        let mut transcript = Transcript::default();
        transcript.live(
            7,
            Some(1),
            &json!({"type":"text","content":"temporary preview"}),
        );
        assert!(rows(&mut transcript)
            .iter()
            .any(|row| row.contains("temporary preview")));
        transcript.apply(response(8, "settled response".into()), Some(7));
        let rendered = rows(&mut transcript).join("\n");
        assert!(!rendered.contains("temporary preview"));
        assert!(rendered.contains("settled response"));
    }
}
