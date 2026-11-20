use std::collections::{HashSet, VecDeque};
use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::markup::render_literal;
use crate::text::shorten;
use crate::theme::{emphasis, ACCENT, MUTED, SUCCESS};

use super::agent::{AgentCard, AgentCards};
use super::tool::{style_card, tool_title, Tool};
use super::{
    body_inset, render_rows, Key, Record, RewindGroupAction, RewindGroupChoice, Row, ToolDetails,
    Transcript, MAX_RECORDS,
};

const INSPECTIONS: [(&str, &str, &str); 5] = [
    ("read_file", "file read", "file reads"),
    ("find_files", "file search", "file searches"),
    ("search_text", "text search", "text searches"),
    ("list_agents", "Agent listing", "Agent listings"),
    ("query_session", "Session query", "Session queries"),
];
const GROUP_DETAIL_ROWS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailKey {
    kind: DetailKind,
    record: Key,
}

impl DetailKey {
    pub(crate) fn rewind_group_action(&self) -> Option<(u64, RewindGroupAction)> {
        let Key::RewindGroup(id) = &self.record else {
            return None;
        };
        let action = match self.kind {
            DetailKind::RewindGroup => RewindGroupAction::Toggle,
            DetailKind::RewindGroupRetry => RewindGroupAction::Retry,
            DetailKind::RewindGroupMore => RewindGroupAction::More,
            _ => return None,
        };
        Some((*id, action))
    }

    fn message_action(kind: DetailKind, record: &Key) -> Self {
        Self {
            kind,
            record: record.clone(),
        }
    }

    pub(super) fn settle(&mut self, preview: &Key, settled: &Key) {
        if self.record == *preview {
            self.record = settled.clone();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailKind {
    Agent,
    Group,
    Reasoning,
    RewindGroup,
    RewindGroupMore,
    RewindGroupRetry,
    Tool,
}

#[derive(Debug, Default)]
pub(super) struct Details {
    pub(super) visible: Vec<VisibleDetail>,
    groups: VecDeque<Key>,
    inspections: VecDeque<Key>,
}

impl Details {
    pub(super) fn rows_for(&self, key: &DetailKey) -> Option<Range<usize>> {
        self.visible
            .iter()
            .find(|item| &item.key == key)
            .map(|item| item.rows.clone())
    }

    pub(super) fn agent_keys(&self) -> impl Iterator<Item = &Key> {
        self.visible
            .iter()
            .filter(|item| item.key.kind == DetailKind::Agent)
            .map(|item| &item.key.record)
    }

    pub(super) fn group_open(&self, records: &[Record]) -> bool {
        records
            .iter()
            .any(|record| self.groups.contains(&record.key))
    }

    pub(super) fn tool_mode(&self, key: &Key, fallback: ToolDetails) -> ToolDetails {
        if self.inspections.contains(key) {
            ToolDetails::Inspection
        } else {
            fallback
        }
    }

    pub(super) fn has_collapsed_detail(&self) -> bool {
        self.visible
            .iter()
            .any(|item| item.key.kind == DetailKind::Group && !item.open)
    }

    pub(super) fn only_reasoning(&self) -> bool {
        self.visible
            .iter()
            .all(|item| item.key.kind == DetailKind::Reasoning)
    }

    pub(super) fn clear_expansions(&mut self) {
        self.groups.clear();
        self.inspections.clear();
    }
}

#[derive(Debug)]
pub(super) struct VisibleDetail {
    agent_link: Option<String>,
    rows: Range<usize>,
    key: DetailKey,
    label: String,
    summary: String,
    open: bool,
}

impl VisibleDetail {
    pub(super) fn group(
        rows: Range<usize>,
        key: &Key,
        group: GroupHeading,
        title_visible: bool,
    ) -> Self {
        let GroupHeading {
            summary,
            open,
            agent_link,
            ..
        } = group;
        let mut item = Self::new(
            rows,
            key,
            DetailKind::Group,
            summary.title,
            summary.composition,
            open,
        );
        if title_visible {
            item.agent_link = agent_link;
        }
        item
    }

    pub(super) fn record(
        rows: Range<usize>,
        key: &Key,
        item: DetailRecord<'_>,
        title_visible: bool,
    ) -> Self {
        match item {
            DetailRecord::Reasoning(excerpt) => Self::new(
                rows,
                key,
                DetailKind::Reasoning,
                "View thinking".into(),
                shorten(
                    &crate::text::single_line(crate::text::cap(excerpt, 256)),
                    66,
                ),
                false,
            ),
            DetailRecord::Agent(agent) => {
                let mut item = Self::new(
                    rows,
                    key,
                    DetailKind::Agent,
                    format!("Agent {}", agent.address),
                    agent.choice_summary(),
                    false,
                );
                if title_visible && agent.receipt_cursor.is_some() {
                    item.agent_link = Some(agent.address.to_owned());
                }
                item
            }

            DetailRecord::Tool(tool) => {
                let (label, summary) = tool.detail_choice();
                Self::new(rows, key, DetailKind::Tool, label, summary, false)
            }
        }
    }

    fn new(
        rows: Range<usize>,
        key: &Key,
        kind: DetailKind,
        label: String,
        summary: String,
        open: bool,
    ) -> Self {
        Self {
            agent_link: None,
            rows,
            key: DetailKey {
                kind,
                record: key.clone(),
            },
            label,
            summary,
            open,
        }
    }
}

pub(super) enum DetailRecord<'a> {
    Reasoning(&'a str),
    Agent(AgentCard<'a>),
    Tool(&'a Tool),
}

impl Transcript {
    pub(super) fn message_action_keys(
        &self,
        record: &Record,
    ) -> (Option<DetailKey>, Option<DetailKey>) {
        let thinking = (!record.reasoning_preview.is_empty())
            .then(|| DetailKey::message_action(DetailKind::Reasoning, &record.key));
        let tool_message = matches!(&record.body, super::Body::Tool(_));
        let expandable = self
            .detail_items
            .visible
            .iter()
            .find(|item| {
                item.key.record == record.key
                    && item.key.kind != DetailKind::Reasoning
                    && (!tool_message || item.key.kind != DetailKind::Group)
            })
            .map(|item| item.key.clone())
            .or_else(|| match &record.body {
                super::Body::Tool(tool) => Some(DetailKey::message_action(
                    if tool.agent_card().is_some() {
                        DetailKind::Agent
                    } else {
                        DetailKind::Tool
                    },
                    &record.key,
                )),
                _ => None,
            });
        (thinking, expandable)
    }

    pub(crate) fn markdown_link_at(&self, row: usize, column: usize) -> Option<&str> {
        let row = match &self.notes {
            Some(notes) if notes.hit(row) => return None,
            Some(notes) => notes.source_row(row),
            None => row,
        };
        self.visible_links
            .iter()
            .find(|(line, link)| *line == row && link.columns.contains(&column))
            .map(|(_, link)| link.url.as_ref())
    }

    pub(crate) fn agent_link_at(&self, row: usize, column: usize, width: usize) -> Option<&str> {
        let (_, action) = crate::theme::CardFrame::agent_action(width)?;
        if !action.contains(&column) {
            return None;
        }
        let row = if let Some(notes) = &self.notes {
            if notes.hit(row) {
                return None;
            }
            notes.source_row(row)
        } else {
            row
        };
        self.detail_items
            .visible
            .iter()
            .find(|item| item.rows.start == row && item.agent_link.is_some())?
            .agent_link
            .as_deref()
    }

    pub(crate) fn detail_at(&self, row: usize) -> Option<&DetailKey> {
        let row = if let Some(notes) = &self.notes {
            if notes.hit(row) {
                return notes.is_heading(row).then_some(&notes.key);
            }
            notes.source_row(row)
        } else {
            row
        };
        self.detail_items
            .visible
            .iter()
            .find(|item| item.rows.contains(&row))
            .map(|item| &item.key)
    }

    pub(crate) fn visible_detail_choices(&self) -> Vec<(DetailKey, String, String)> {
        let mut choices = self
            .detail_items
            .visible
            .iter()
            .rev()
            .map(|item| (item.key.clone(), item.label.clone(), item.summary.clone()))
            .collect::<Vec<_>>();
        let rewind_groups: Vec<RewindGroupChoice> = self.rewind_group_choices();
        for group in rewind_groups.into_iter().rev() {
            let action = if group.expanded { "Collapse" } else { "Expand" };
            let key = Key::RewindGroup(group.rewind_event_id);
            choices.push((
                DetailKey {
                    kind: DetailKind::RewindGroup,
                    record: key.clone(),
                },
                format!("{action} · {}", group.label),
                format!("Agent {} · {}", group.agent, group.summary),
            ));
            let action = if group.can_retry {
                Some((DetailKind::RewindGroupRetry, "Retry loading"))
            } else if group.can_load_more {
                Some((DetailKind::RewindGroupMore, "Load earlier"))
            } else {
                None
            };
            if let Some((kind, label)) = action {
                choices.push((
                    DetailKey { kind, record: key },
                    format!("{label} · {}", group.label),
                    format!("Agent {} · {}", group.agent, group.summary),
                ));
            }
        }
        choices
    }

    #[cfg(test)]
    pub(crate) fn latest_notes(&self) -> Option<DetailKey> {
        self.detail_items
            .visible
            .iter()
            .rev()
            .find(|item| item.key.kind == DetailKind::Reasoning)
            .map(|item| item.key.clone())
    }

    pub(super) fn notes_at(&self, key: &DetailKey) -> Option<(&str, Range<usize>)> {
        if key.kind != DetailKind::Reasoning {
            return None;
        }
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        let record = window
            .records
            .iter()
            .find(|record| record.key == key.record)?;
        Some((&record.reasoning_preview, self.detail_items.rows_for(key)?))
    }

    pub(crate) fn toggle_in_view(&mut self, key: &DetailKey, width: usize, height: usize) -> bool {
        if !self.toggle_detail(key) {
            return false;
        }
        let anchor = super::Anchor {
            key: key.record.clone(),
            line: if key.kind == DetailKind::Group {
                super::AnchorLine::Group(0)
            } else {
                super::AnchorLine::Body(0)
            },
        };
        let layout = super::Layout {
            width,
            tools: self.details,
            assistant: self.assistant_view,
            agents: &self.agent_cards,
            items: &self.detail_items,
        };
        if let Some(reading) = &mut self.reading {
            let opening = match key.kind {
                DetailKind::Agent => self.agent_cards.is_open(&key.record),
                DetailKind::Group => self.detail_items.groups.contains(&key.record),
                _ => false,
            };
            let reveal = self
                .detail_items
                .rows_for(key)
                .filter(|_| opening)
                .map_or(0, |rows| (rows.end + 3).saturating_sub(height));
            if reveal > 0 {
                let start = reading.window.position_of(&reading.anchor, layout);
                let heading = reading.window.position_of(&anchor, layout);
                let next = (start + reveal).min(heading);
                if next > start {
                    if let Some(anchor) = reading.window.anchor_at(next, layout) {
                        reading.anchor = anchor;
                    }
                }
            }
            return true;
        }
        let top = self.recent.row_count(layout).saturating_sub(height);
        if self.recent.position_of(&anchor, layout) < top {
            self.reading = Some(super::Reading {
                window: self.recent.clone(),
                anchor,
                newer: VecDeque::new(),
                unseen: false,
                manual: true,
            });
        }
        true
    }

    pub(crate) fn toggle_detail(&mut self, key: &DetailKey) -> bool {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        match key.kind {
            DetailKind::Agent => self.toggle_agent_record(&key.record),
            DetailKind::Tool if window.contains(&key.record) => {
                let group_to_open = groups(&window.records)
                    .into_iter()
                    .find(|span| {
                        window.records[span.range.clone()]
                            .iter()
                            .any(|record| record.key == key.record)
                    })
                    .and_then(|span| {
                        let records = &window.records[span.range];
                        (!self.detail_items.group_open(records))
                            .then(|| DetailKey::message_action(DetailKind::Group, &records[0].key))
                    });
                let group_was_collapsed = group_to_open.is_some();
                if let Some(group_key) = group_to_open {
                    self.toggle_detail(&group_key);
                }
                if group_was_collapsed {
                    if !self.detail_items.inspections.contains(&key.record) {
                        remember(&mut self.detail_items.inspections, &key.record);
                    }
                } else {
                    if let Some(index) = self
                        .detail_items
                        .inspections
                        .iter()
                        .position(|opened| opened == &key.record)
                    {
                        self.detail_items.inspections.remove(index);
                    } else {
                        remember(&mut self.detail_items.inspections, &key.record);
                    }
                }
                true
            }
            DetailKind::Tool
            | DetailKind::Reasoning
            | DetailKind::RewindGroup
            | DetailKind::RewindGroupMore
            | DetailKind::RewindGroupRetry => false,
            DetailKind::Group => {
                let Some(span) = groups(&window.records).into_iter().find(|span| {
                    window.records[span.range.clone()]
                        .iter()
                        .any(|record| record.key == key.record)
                }) else {
                    return false;
                };
                let records = &window.records[span.range];
                let open = !self.detail_items.group_open(records);
                if open {
                    remember(&mut self.detail_items.groups, &key.record);
                    self.agent_cards.set_details(
                        true,
                        records
                            .iter()
                            .filter(|r| r.tool().and_then(Tool::agent_card).is_some())
                            .map(|r| &r.key),
                    );
                } else {
                    self.detail_items
                        .groups
                        .retain(|opened| !records.iter().any(|record| &record.key == opened));
                }
                if let Some(item) = self.detail_items.visible.iter_mut().find(|item| {
                    item.key.kind == DetailKind::Group
                        && records.iter().any(|record| record.key == item.key.record)
                }) {
                    item.open = open;
                }
                true
            }
        }
    }

    pub(super) fn toggle_group_details(&mut self, open: bool) {
        if !open {
            self.detail_items.clear_expansions();
        }
        if open {
            let records = &self
                .reading
                .as_ref()
                .map_or(&self.recent, |reading| &reading.window)
                .records;
            for group in groups(records) {
                let members = &records[group.range];
                if self.detail_items.visible.iter().any(|item| {
                    item.key.kind == DetailKind::Group
                        && members.iter().any(|r| r.key == item.key.record)
                }) {
                    self.agent_cards.set_details(
                        true,
                        members
                            .iter()
                            .filter(|r| r.tool().and_then(Tool::agent_card).is_some())
                            .map(|r| &r.key),
                    );
                }
            }
        }
        for item in &mut self.detail_items.visible {
            if item.key.kind != DetailKind::Group {
                continue;
            }
            if open && !item.open {
                remember(&mut self.detail_items.groups, &item.key.record);
            }
            item.open = open;
        }
    }
}

fn remember(expanded: &mut VecDeque<Key>, key: &Key) {
    if expanded.len() == MAX_RECORDS {
        expanded.pop_front();
    }
    expanded.push_back(key.clone());
}

pub(super) struct GroupSpan {
    pub(super) range: Range<usize>,
    pub(super) summary: GroupSummary,
}

pub(super) struct GroupSummary {
    title: String,
    composition: String,
    listing: String,
    kind: GroupKind,
}

#[derive(PartialEq, Eq)]
enum GroupKind {
    Delegation { delivery: usize, checks: usize },
    Listings,
    Inspections,
}

pub(super) struct GroupHeading {
    agent_link: Option<String>,
    summary: GroupSummary,
    pub(super) open: bool,
    pub(super) rows: Vec<Row>,
}

impl GroupHeading {
    pub(super) fn from_span(
        span: GroupSpan,
        records: &[Record],
        open: bool,
        width: usize,
        agents: &AgentCards,
    ) -> Self {
        let summary = span.summary;
        if let GroupKind::Delegation { delivery, checks } = summary.kind {
            let latest = &records[delivery];
            if let Some(card) = latest.tool().and_then(Tool::agent_card) {
                let (_, facts) = agents.display(&latest.key, latest.position, card.address);
                let rows = card.delegation_header(width, open, facts, checks);
                return Self {
                    agent_link: Some(card.address.to_owned()),
                    summary,
                    open,
                    rows,
                };
            }
        }
        Self::new(summary, open, width)
    }

    pub(super) fn new(summary: GroupSummary, open: bool, width: usize) -> Self {
        let style = if summary.kind == GroupKind::Listings {
            Style::new().bg(crate::theme::AGENT_SURFACE)
        } else {
            crate::theme::panel()
        };
        let rows = super::message::padded_panel(width, style, |width| {
            let inset = body_inset(width);
            let columns = width.saturating_sub(usize::from(inset));
            let mut spans = Vec::new();
            if inset > 0 {
                spans.push(Span::styled(
                    if open { "▾ " } else { "▸ " },
                    Style::new().fg(MUTED),
                ));
            }
            let status = " · ok";
            let title = if columns > status.width() {
                shorten(&summary.title, columns - status.width())
            } else {
                shorten(&summary.title, columns)
            };
            spans.push(Span::styled(title, emphasis(ACCENT)));
            if columns > status.width() {
                spans.push(Span::styled(status, Style::new().fg(SUCCESS)));
            }
            let mut rows = vec![Row {
                line: Line::from(spans),
                inset,
                links: Vec::new(),
            }];
            for detail in [&summary.composition, &summary.listing] {
                if !detail.is_empty() {
                    rows.extend(render_rows(
                        render_literal(detail, Style::default()),
                        width,
                        GROUP_DETAIL_ROWS,
                        inset,
                    ));
                }
            }
            if summary.kind == GroupKind::Listings {
                super::paint_card(&mut rows, width, crate::theme::AGENT_SURFACE);
            } else {
                style_card(&mut rows);
            }
            rows
        });
        Self {
            agent_link: None,
            summary,
            open,
            rows,
        }
    }
}

pub(super) fn groups(records: &[Record]) -> Vec<GroupSpan> {
    let mut groups = Vec::new();
    let mut index = 0;
    while let Some(first) = records.get(index) {
        let same_turn = |r: &Record| {
            first.turn.is_some()
                && first.owner.is_some()
                && r.turn == first.turn
                && r.owner == first.owner
        };
        let next_action = |after: usize| {
            let mut next = after + 1;
            while records
                .get(next)
                .is_some_and(|r| same_turn(r) && r.reasoning_only())
            {
                next += 1;
            }
            next
        };
        if let Some(create) = first
            .tool()
            .and_then(Tool::agent_card)
            .filter(|card| card.created())
        {
            let next = next_action(index);
            if let Some(send) = records
                .get(next)
                .filter(|r| same_turn(r))
                .and_then(Record::tool)
                .and_then(Tool::agent_card)
                .filter(|card| card.delivered() && card.address == create.address)
            {
                let previous_end = groups.last().map_or(0, |group: &GroupSpan| group.range.end);
                let mut start = index;
                while start > previous_end
                    && same_turn(&records[start - 1])
                    && records[start - 1].reasoning_only()
                {
                    start -= 1;
                }
                let mut last = next;
                let mut checks = 0;
                loop {
                    let check = next_action(last);
                    if !records
                        .get(check)
                        .filter(|r| same_turn(r))
                        .and_then(Record::tool)
                        .is_some_and(|t| {
                            t.checks_delegation(
                                first.owner.as_deref().unwrap_or_default(),
                                send.address,
                            )
                        })
                    {
                        break;
                    }
                    last = check;
                    checks += 1;
                }
                groups.push(GroupSpan {
                    range: start..last + 1,
                    summary: GroupSummary {
                        title: format!("Agent {}", send.address),
                        composition: send.choice_summary(),
                        listing: String::new(),
                        kind: GroupKind::Delegation {
                            delivery: next,
                            checks,
                        },
                    },
                });
                index = last + 1;
                continue;
            }
        }
        let start = index;
        if let Some(name) = group_name(first) {
            loop {
                let next = next_action(index);
                let Some(next_name) = records
                    .get(next)
                    .filter(|r| same_turn(r))
                    .and_then(group_name)
                else {
                    break;
                };
                if next > index + 1
                    && !(name == "list_agents"
                        && next_name == name
                        && group_name(&records[index]) == Some(name))
                {
                    break;
                }
                index = next;
            }
            if index > start {
                groups.push(GroupSpan {
                    range: start..index + 1,
                    summary: group_summary(name, &records[start..=index]),
                });
            }
        }
        index += 1;
    }
    groups
}

fn group_name(record: &Record) -> Option<&str> {
    record.turn.filter(|&turn| turn > 0)?;
    record.owner.as_ref()?;
    record.tool()?.group_name()
}

fn group_summary(first: &str, records: &[Record]) -> GroupSummary {
    let count = records.iter().filter_map(group_name).count();
    let mixed = records
        .iter()
        .filter_map(group_name)
        .any(|name| name != first);
    let title = if mixed {
        format!("Read-only activity · {count} calls")
    } else {
        group_title(first, records)
    };
    let mut parts = Vec::new();
    if mixed {
        for (name, one, many) in INSPECTIONS {
            let count = records
                .iter()
                .filter(|record| group_name(record) == Some(name))
                .count();
            if count > 0 {
                parts.push(format!("{count} {}", if count == 1 { one } else { many }));
            }
        }
    }
    let listing = records
        .iter()
        .rev()
        .find(|record| group_name(record) == Some("list_agents"))
        .and_then(|record| record.tool()?.agent_listing())
        .map_or_else(String::new, |(caption, detail)| {
            if detail.is_empty() {
                caption.to_owned()
            } else {
                format!("Latest · {caption}\n{detail}")
            }
        });
    GroupSummary {
        title,
        composition: parts.join(" · "),
        listing,
        kind: if !mixed && first == "list_agents" {
            GroupKind::Listings
        } else {
            GroupKind::Inspections
        },
    }
}

fn group_title(name: &str, records: &[Record]) -> String {
    let count = records.iter().filter_map(group_name).count();
    if name == "read_file" {
        let unique: HashSet<_> = records
            .iter()
            .filter_map(|record| record.tool()?.read_path())
            .collect();
        if unique.len() == count {
            format!("Read {count} files")
        } else {
            format!("Read file ×{count}")
        }
    } else {
        format!("{} ×{count}", tool_title(name))
    }
}

#[cfg(test)]
mod tests;
