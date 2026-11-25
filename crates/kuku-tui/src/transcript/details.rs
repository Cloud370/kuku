use std::collections::VecDeque;
use std::ops::Range;

use crate::text::shorten;

use super::agent::AgentCard;
use super::tool::Tool;
use super::{
    Key, MAX_RECORDS, Record, RewindGroupAction, RewindGroupChoice, Row, ToolDetails, Transcript,
    render_rows,
};

mod activity;
pub(super) use activity::footer as activity_footer;
mod groups;
use groups::GroupKind;
pub(super) use groups::{GroupHeading, GroupSummary, compact_file_rows, groups};
mod inspection;
mod output;
pub(super) use inspection::inspection_composition;
pub(crate) use output::InlineOutputEffect;
pub(super) use output::{InlineOutput, InlineOutputAction, OutputTarget};

const GROUP_DETAIL_ROWS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailKey {
    kind: DetailKind,
    record: Key,
}

impl DetailKey {
    pub(crate) fn is_inline_output(&self) -> bool {
        matches!(self.kind, DetailKind::InlineOutput(..))
    }

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

    pub(super) fn for_record(&self, key: &Key) -> bool {
        &self.record == key
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
    CommandArguments,
    Compaction,
    Group,
    Output(OutputTarget),
    InlineOutput(OutputTarget, InlineOutputAction),
    Reasoning,
    RewindGroup,
    RewindGroupMore,
    RewindGroupRetry,
    Script(ScriptDetail),
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScriptDetail {
    Action(usize),
    Return,
    Source,
}

impl ScriptDetail {
    pub(super) fn action(self) -> Option<usize> {
        match self {
            Self::Action(index) => Some(index),
            Self::Return | Self::Source => None,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Action(_) => "Host action",
            Self::Return => "Output",
            Self::Source => "Input",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Details {
    pub(super) visible: Vec<VisibleDetail>,
    groups: VecDeque<GroupChoice>,
    inspections: VecDeque<Key>,
    command_arguments: VecDeque<Key>,
    script_detail: Option<DetailKey>,
    inline_output: Option<InlineOutput>,
}

#[derive(Debug)]
struct GroupChoice {
    // A group may outlive its first loaded member. Alias keys keep its explicit
    // choice shared between the recent and paged reading windows.
    keys: VecDeque<Key>,
    open: bool,
}

impl GroupChoice {
    fn matches(&self, records: &[Record]) -> bool {
        records.iter().any(|record| self.keys.contains(&record.key))
    }
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

    pub(super) fn group_open(
        &self,
        records: &[Record],
        automatic: bool,
        tools: ToolDetails,
    ) -> bool {
        self.group_choice(records).map_or_else(
            || tools != ToolDetails::Collapsed && automatic,
            |choice| choice.open,
        )
    }

    fn group_choice(&self, records: &[Record]) -> Option<&GroupChoice> {
        self.groups
            .iter()
            .rev()
            .find(|choice| choice.matches(records))
    }

    pub(super) fn has_group_choice(&self, key: &Key) -> bool {
        self.groups.iter().any(|choice| choice.keys.contains(key))
    }

    pub(super) fn has_explicit_group(&self, records: &[Record]) -> bool {
        self.group_choice(records).is_some()
    }

    pub(super) fn has_member_inspection(&self, key: &Key) -> bool {
        self.inspections.contains(key) || self.script_detail(key).is_some()
    }

    pub(super) fn preserve_group_choice(
        &mut self,
        records: &[Record],
        retained: impl Fn(&Record) -> bool,
        survivor: &Key,
    ) {
        let Some(choice) = self
            .groups
            .iter_mut()
            .rev()
            .find(|choice| choice.matches(records))
        else {
            return;
        };
        if records
            .iter()
            .any(|record| retained(record) && choice.keys.contains(&record.key))
        {
            return;
        }
        choice.keys.push_back(survivor.clone());
        self.trim_group_choices();
    }

    fn trim_group_choices(&mut self) {
        let mut count = self
            .groups
            .iter()
            .map(|choice| choice.keys.len())
            .sum::<usize>();
        while count > MAX_RECORDS {
            let choice = self.groups.front_mut().expect("group choices remain");
            choice.keys.pop_front();
            if choice.keys.is_empty() {
                self.groups.pop_front();
            }
            count -= 1;
        }
    }

    fn set_group(&mut self, records: &[Record], key: &Key, open: bool) {
        let mut keys = VecDeque::new();
        self.groups.retain_mut(|choice| {
            if choice.matches(records) {
                keys.append(&mut choice.keys);
                false
            } else {
                true
            }
        });
        keys.retain(|member| member != key);
        keys.push_back(key.clone());
        self.groups.push_back(GroupChoice { keys, open });
        self.trim_group_choices();
        if let Some(item) = self.visible.iter_mut().find(|item| {
            item.key.kind == DetailKind::Group
                && records.iter().any(|record| record.key == item.key.record)
        }) {
            item.open = open;
        }
    }

    pub(super) fn tool_mode(&self, key: &Key, fallback: ToolDetails) -> ToolDetails {
        if self.inspections.contains(key) {
            if fallback == ToolDetails::CompactFile {
                ToolDetails::ExpandedFile
            } else if self.command_arguments.contains(key) {
                ToolDetails::InspectionWithArguments
            } else {
                ToolDetails::Inspection
            }
        } else {
            fallback
        }
    }

    pub(super) fn script_detail(&self, record: &Key) -> Option<ScriptDetail> {
        let key = self
            .script_detail
            .as_ref()
            .filter(|key| key.for_record(record))?;
        match key.kind {
            DetailKind::Script(detail) => Some(detail),
            _ => None,
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
        self.command_arguments.clear();
        self.script_detail = None;
        self.inline_output = None;
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
    pub(super) fn command_arguments(row: usize, key: &Key, open: bool) -> Self {
        Self::new(
            row..row + 1,
            key,
            DetailKind::CommandArguments,
            Row::arguments_label(open).into(),
            String::new(),
            open,
        )
    }

    pub(super) fn script_detail(
        row: usize,
        key: &Key,
        tool: &Tool,
        detail: ScriptDetail,
        open: bool,
    ) -> Option<Self> {
        let (subject, description) = match detail {
            ScriptDetail::Action(index) => tool.script_action_description(index)?,
            _ => (detail.label(), ""),
        };
        Some(Self::new(
            row..row + 1,
            key,
            DetailKind::Script(detail),
            format!("{} {subject}", if open { "Collapse" } else { "Expand" }),
            crate::text::single_line(description),
            open,
        ))
    }

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
            DetailRecord::Compaction(summary, open) => Self::new(
                rows,
                key,
                DetailKind::Compaction,
                if open {
                    "Collapse checkpoint"
                } else {
                    "Expand checkpoint"
                }
                .into(),
                shorten(
                    &crate::text::single_line(crate::text::cap(summary, 256)),
                    66,
                ),
                open,
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
    Compaction(&'a str, bool),
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
                    && !matches!(
                        item.key.kind,
                        DetailKind::Reasoning
                            | DetailKind::Script(_)
                            | DetailKind::Output(_)
                            | DetailKind::InlineOutput(..)
                            | DetailKind::CommandArguments
                    )
                    && (!tool_message || item.key.kind != DetailKind::Group)
            })
            .map(|item| item.key.clone())
            .or_else(|| match &record.body {
                super::Body::Compaction(compaction) => compaction
                    .summary()
                    .map(|_| DetailKey::message_action(DetailKind::Compaction, &record.key)),
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
        self.detail_items
            .visible
            .iter()
            .find(|item| item.rows.start == row && item.agent_link.is_some())?
            .agent_link
            .as_deref()
    }

    pub(crate) fn detail_at(&self, row: usize) -> Option<&DetailKey> {
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

    pub(super) fn notes_at(&self, key: &DetailKey) -> Option<(&str, bool)> {
        if key.kind != DetailKind::Reasoning {
            return None;
        }
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        self.detail_items.rows_for(key)?;
        let index = window
            .records
            .iter()
            .position(|record| record.key == key.record)?;
        let in_timeline = groups(&window.records)
            .iter()
            .any(|group| group.range.contains(&index));
        Some((&window.records[index].reasoning_preview, in_timeline))
    }

    pub(crate) fn toggle_in_view(&mut self, key: &DetailKey, width: usize, height: usize) -> bool {
        let retain_row = self
            .detail_items
            .rows_for(key)
            .filter(|_| match key.kind {
                DetailKind::Script(_) => true,
                DetailKind::Tool => self
                    .reading
                    .as_ref()
                    .map_or(&self.recent, |reading| &reading.window)
                    .records
                    .iter()
                    .find(|record| key.for_record(&record.key))
                    .and_then(Record::tool)
                    .is_some_and(|tool| tool.name() == "run_js"),
                _ => false,
            })
            .map(|rows| rows.start);
        if !self.toggle_detail(key) {
            return false;
        }
        let anchor = super::Anchor {
            key: key.record.clone(),
            line: match key.kind {
                DetailKind::Group => super::AnchorLine::Group(0),
                DetailKind::Script(detail) => super::AnchorLine::Script(detail),
                DetailKind::CommandArguments => super::AnchorLine::CommandArguments,
                _ => super::AnchorLine::Body(0),
            },
        };
        let layout = super::Layout {
            width,
            tools: self.details,
            assistant: self.assistant_view,
            agents: &self.agent_cards,
            items: &self.detail_items,
            notes: self.notes.as_ref(),
        };
        if let Some(row) = retain_row {
            if let Some(reading) = &mut self.reading {
                let top = reading
                    .window
                    .position_of(&anchor, layout)
                    .saturating_sub(row);
                reading.anchor = reading.window.anchor_at(top, layout).unwrap_or(anchor);
                return true;
            }
            let top = self.recent.position_of(&anchor, layout).saturating_sub(row);
            if self.recent.row_count(layout).saturating_sub(height) != top {
                let anchor = self.recent.anchor_at(top, layout).unwrap_or(anchor);
                self.reading = Some(super::Reading {
                    window: self.recent.reading_copy(),
                    anchor,
                    newer: None,
                    rebase_after_page: false,
                    visible_through: None,
                    unseen: false,
                    manual: true,
                });
            }
            return true;
        }
        if let Some(reading) = &mut self.reading {
            let opening = match key.kind {
                DetailKind::Agent => self.agent_cards.is_open(&key.record),
                DetailKind::Group => self
                    .detail_items
                    .visible
                    .iter()
                    .any(|item| item.key == *key && item.open),
                DetailKind::Tool | DetailKind::Compaction => {
                    self.detail_items.inspections.contains(&key.record)
                }
                DetailKind::Script(_) => self.detail_items.script_detail.as_ref() == Some(key),
                DetailKind::CommandArguments => {
                    self.detail_items.command_arguments.contains(&key.record)
                }
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
                window: self.recent.reading_copy(),
                anchor,
                newer: None,
                rebase_after_page: false,
                visible_through: None,
                unseen: false,
                manual: true,
            });
        }
        true
    }

    pub(crate) fn toggle_detail(&mut self, key: &DetailKey) -> bool {
        let in_window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window)
            .contains(&key.record);
        match key.kind {
            DetailKind::Agent => {
                let opening = !self.agent_cards.is_open(&key.record);
                let group_was_collapsed = self.open_containing_group(&key.record, opening);
                if group_was_collapsed && self.agent_cards.is_open(&key.record) {
                    true
                } else {
                    self.toggle_agent_record(&key.record)
                }
            }
            DetailKind::Tool | DetailKind::Compaction if in_window => {
                let opening = !self.detail_items.inspections.contains(&key.record);
                let group_was_collapsed = self.open_containing_group(&key.record, opening);
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
                        if self.detail_items.script_detail(&key.record).is_some() {
                            self.detail_items.script_detail = None;
                        }
                        self.detail_items.close_inline_output(&key.record);
                        self.detail_items
                            .command_arguments
                            .retain(|opened| opened != &key.record);
                    } else {
                        remember(&mut self.detail_items.inspections, &key.record);
                    }
                }
                true
            }
            DetailKind::Script(_) if in_window => {
                let opening = self.detail_items.script_detail.as_ref() != Some(key);
                if opening {
                    self.open_containing_group(&key.record, true);
                }
                self.detail_items.close_inline_output(&key.record);
                self.detail_items.script_detail = opening.then(|| key.clone());
                true
            }
            DetailKind::CommandArguments
                if in_window && self.detail_items.inspections.contains(&key.record) =>
            {
                if let Some(index) = self
                    .detail_items
                    .command_arguments
                    .iter()
                    .position(|opened| opened == &key.record)
                {
                    self.detail_items.command_arguments.remove(index);
                } else {
                    self.open_containing_group(&key.record, true);
                    remember(&mut self.detail_items.command_arguments, &key.record);
                }
                true
            }
            DetailKind::Tool
            | DetailKind::Compaction
            | DetailKind::Reasoning
            | DetailKind::Output(_)
            | DetailKind::InlineOutput(..)
            | DetailKind::CommandArguments
            | DetailKind::Script(_)
            | DetailKind::RewindGroup
            | DetailKind::RewindGroupMore
            | DetailKind::RewindGroupRetry => false,
            DetailKind::Group => {
                let window = self
                    .reading
                    .as_ref()
                    .map_or(&self.recent, |reading| &reading.window);
                let Some(span) = groups(&window.records).into_iter().find(|span| {
                    window.records[span.range.clone()]
                        .iter()
                        .any(|record| record.key == key.record)
                }) else {
                    return false;
                };
                let records = &window.records[span.range];
                let open = !self.detail_items.group_open(
                    records,
                    window.automatic_group(records),
                    self.details,
                );
                self.detail_items.set_group(records, &key.record, open);
                true
            }
        }
    }

    fn open_containing_group(&mut self, key: &Key, opening: bool) -> bool {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        let Some(mut span) = groups(&window.records).into_iter().find(|span| {
            window.records[span.range.clone()]
                .iter()
                .any(|record| &record.key == key)
        }) else {
            return false;
        };
        let records = &window.records[span.range];
        let (open, _) =
            window.group_display(&mut span.summary, records, &self.detail_items, self.details);
        if open {
            // An explicit child inspection in an auto-open group must remain
            // readable after newer activity takes over the automatic opening.
            if opening && self.detail_items.group_choice(records).is_none() {
                self.detail_items.set_group(records, key, true);
            }
            return false;
        }
        // A visible collapsed preview is independently inspectable. Opening
        // that row must not undo the user's deliberate fold of the whole group.
        let preview_visible = span.summary.activity_preview().is_some_and(|preview| {
            records
                .iter()
                .zip(preview)
                .any(|(record, count)| &record.key == key && *count > 0)
        });
        if preview_visible {
            return false;
        }
        self.detail_items.set_group(records, key, true);
        true
    }

    pub(super) fn toggle_group_details(&mut self, open: bool) {
        if !open {
            self.detail_items.clear_expansions();
        }
        let records = &self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window)
            .records;
        for group in groups(records) {
            let members = &records[group.range];
            if let Some(key) = self
                .detail_items
                .visible
                .iter()
                .find(|item| {
                    item.key.kind == DetailKind::Group
                        && members.iter().any(|r| r.key == item.key.record)
                })
                .map(|item| item.key.record.clone())
            {
                if open {
                    self.agent_cards.set_details(
                        true,
                        members
                            .iter()
                            .filter(|r| r.tool().and_then(Tool::agent_card).is_some())
                            .map(|r| &r.key),
                    );
                }
                self.detail_items.set_group(members, &key, open);
            }
        }
        for item in &mut self.detail_items.visible {
            if item.key.kind != DetailKind::Group {
                continue;
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

#[cfg(test)]
mod tests;
