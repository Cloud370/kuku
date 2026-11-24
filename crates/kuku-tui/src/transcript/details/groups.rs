use std::collections::HashSet;
use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::markup::render_literal;
use crate::text::shorten;
use crate::theme::{ACCENT, CardFrame, MUTED, SUCCESS, emphasis};
use crate::transcript::agent::AgentCards;
use crate::transcript::tool::{Tool, tool_title};
use crate::transcript::{Record, Row, render_rows};

use super::inspection::{INSPECTIONS, inspection_composition};
use super::{GROUP_DETAIL_ROWS, activity};

pub(in crate::transcript) fn compact_file_rows(records: &[Record]) -> Vec<bool> {
    let mut compact = vec![false; records.len()];
    let mut start = 0;
    while start < records.len() {
        let first = &records[start];
        let eligible = |record: &&Record| {
            first.turn.is_some()
                && record.turn == first.turn
                && record.owner == first.owner
                && record
                    .tool()
                    .is_some_and(|tool| tool.compact_file_change() || tool.group_name().is_some())
        };
        let length = records[start..].iter().take_while(eligible).count();
        let run = &records[start..start + length];
        let changes = || {
            run.iter()
                .map(|record| record.tool().is_some_and(|tool| tool.compact_file_change()))
        };
        let multiple_changes = changes().filter(|&changed| changed).nth(1).is_some();
        if length >= 3 || multiple_changes {
            for (slot, changed) in compact[start..start + length].iter_mut().zip(changes()) {
                *slot = changed;
            }
        }
        start += length.max(1);
    }
    compact
}

pub(in crate::transcript) struct GroupSpan {
    pub(in crate::transcript) range: Range<usize>,
    pub(in crate::transcript) summary: GroupSummary,
}

pub(in crate::transcript) struct GroupSummary {
    pub(super) title: String,
    pub(super) composition: String,
    pub(super) listing: String,
    pub(super) preview: Option<String>,
    pub(super) kind: GroupKind,
}

impl GroupSummary {
    pub(in crate::transcript) fn framed(&self) -> bool {
        matches!(self.kind, GroupKind::Delegation { .. })
    }

    pub(in crate::transcript) fn activity_remaining(&self) -> usize {
        match &self.kind {
            GroupKind::WorkspaceActivity { remaining, .. } => *remaining,
            _ => 0,
        }
    }

    pub(in crate::transcript) fn activity_preview(&self) -> Option<&[usize]> {
        match &self.kind {
            GroupKind::WorkspaceActivity { preview, .. } => Some(preview),
            _ => None,
        }
    }
}

#[derive(PartialEq, Eq)]
pub(super) enum GroupKind {
    Delegation {
        creation: usize,
        delivery: usize,
        checks: usize,
    },
    Inspections,
    WorkspaceActivity {
        preview: Vec<usize>,
        remaining: usize,
    },
}

pub(in crate::transcript) struct GroupHeading {
    pub(super) agent_link: Option<String>,
    pub(super) summary: GroupSummary,
    pub(in crate::transcript) open: bool,
    pub(in crate::transcript) rows: Vec<Row>,
}

impl GroupHeading {
    pub(in crate::transcript) fn from_span(
        span: GroupSpan,
        records: &[Record],
        open: bool,
        width: usize,
        agents: &AgentCards,
    ) -> Self {
        let summary = span.summary;
        if let GroupKind::Delegation {
            creation,
            delivery,
            checks,
        } = summary.kind
        {
            let latest = &records[delivery];
            if let Some(card) = latest.tool().and_then(Tool::agent_card) {
                let (_, facts) = agents.display(&latest.key, latest.position, card.address);
                let creation = records[creation].tool().and_then(Tool::agent_card);
                let rows = card.delegation_header(width, open, facts, checks, creation.as_ref());
                return Self {
                    agent_link: Some(card.address.to_owned()),
                    summary,
                    open,
                    rows,
                };
            }
        }
        Self::new(summary, open, width, &records[span.range])
    }

    fn new(summary: GroupSummary, open: bool, width: usize, records: &[Record]) -> Self {
        let frame = CardFrame::timeline(width);
        if !summary.framed() {
            return activity::heading(summary, open, records, &frame);
        }
        let columns = frame.title_width();
        let marker = if open { "▾ " } else { "▸ " };
        let status = " · ok";
        let mut title = Line::default();
        if columns > marker.width() {
            title.push_span(Span::styled(marker, Style::new().fg(MUTED)));
        }
        let room = columns.saturating_sub(title.width());
        let show_status = room > status.width();
        title.push_span(Span::styled(
            shorten(
                &summary.title,
                room - usize::from(show_status) * status.width(),
            ),
            emphasis(ACCENT),
        ));
        if show_status {
            title.push_span(Span::styled(status, Style::new().fg(SUCCESS)));
        }
        let mut rows = vec![Row::plain(frame.title_line(title))];
        let composition = match (open, summary.preview.as_deref()) {
            (true, Some(_)) => "",
            (false, Some(preview)) => preview,
            (_, None) => &summary.composition,
        };
        let listing = if open { "" } else { &summary.listing };
        if open || !composition.is_empty() || !listing.is_empty() {
            rows.extend(Row::frame_padding(&frame));
        }
        for detail in [composition, listing] {
            if !detail.is_empty() {
                rows.extend(
                    render_rows(
                        render_literal(detail, Style::default()),
                        frame.content_width(),
                        GROUP_DETAIL_ROWS,
                        0,
                    )
                    .map(|row| row.framed(&frame)),
                );
            }
        }
        if !open {
            rows.extend(Row::frame_end(&frame));
        }
        Self {
            agent_link: None,
            summary,
            open,
            rows,
        }
    }
}

pub(in crate::transcript) fn groups(records: &[Record]) -> Vec<GroupSpan> {
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
                        preview: None,
                        kind: GroupKind::Delegation {
                            creation: index,
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
        if first.tool().is_some_and(Tool::workspace_activity) {
            let length = 1 + records[start + 1..]
                .iter()
                .take_while(|record| {
                    same_turn(record) && record.tool().is_some_and(Tool::workspace_activity)
                })
                .count();
            let range = start..start + length;
            let activity = &records[range.clone()];
            if length > 1
                && activity
                    .iter()
                    .filter_map(Record::tool)
                    .any(|tool| tool.group_name().is_none())
            {
                groups.push(GroupSpan {
                    range: range.clone(),
                    summary: activity::summary(activity),
                });
                index = range.end;
                continue;
            }
        }
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
            let script_batch = name == "run_js"
                && first.tool().is_some_and(|tool| {
                    INSPECTIONS
                        .iter()
                        .map(|(operation, _, _)| tool.inspection_count(operation))
                        .fold(0, u64::saturating_add)
                        > 1
                });
            if index > start || script_batch {
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
        .any(|name| name != first || name == "run_js");
    let title = if mixed {
        format!("Read-only activity · {count} calls")
    } else {
        group_title(first, records)
    };
    let listing = records
        .iter()
        .rev()
        .find(|record| group_name(record) == Some("list_agents"))
        .and_then(|record| record.tool()?.agent_listing());
    let composition = if mixed {
        inspection_composition(records.iter().filter_map(Record::tool), listing.is_some())
    } else {
        String::new()
    };
    let preview = if mixed && count == 2 {
        let results = records
            .iter()
            .filter_map(|record| record.tool()?.inspection_result())
            .collect::<Vec<_>>();
        (results.len() + usize::from(listing.is_some()) == count).then(|| results.join("\n"))
    } else {
        None
    };
    GroupSummary {
        title,
        composition,
        listing: listing.unwrap_or_default(),
        preview,
        kind: GroupKind::Inspections,
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
