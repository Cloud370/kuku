use kuku_client::HistoryPage;
use serde_json::Value;

use super::agent::AgentCards;
use super::details::{groups, DetailRecord, Details, GroupHeading};
use super::{
    Anchor, AnchorLine, AssistantView, Body, HistoryRequest, Key, Record, RenderCache, Row,
    ToolDetails, MAX_LIVE_RECORDS, MAX_RECORDS,
};

#[derive(Debug, Clone, Default)]
pub(super) struct Window {
    pub(super) records: Vec<Record>,
    pub(super) older: Option<u64>,
}

impl Window {
    pub(super) fn from_page(page: HistoryPage) -> Self {
        let mut window = Self {
            older: page.next_before,
            ..Self::default()
        };
        for record in page.entries.into_iter().filter_map(Record::from_entry) {
            window.insert(record);
        }
        window
    }

    pub(super) fn contains(&self, key: &Key) -> bool {
        self.records.iter().any(|record| &record.key == key)
    }

    pub(super) fn remove(&mut self, key: &Key) {
        self.records.retain(|record| &record.key != key);
    }

    pub(super) fn insert(&mut self, record: Record) {
        if let Some(previous) = self.records.iter_mut().find(|old| old.key == record.key) {
            previous.merge(record);
        } else {
            self.records.push(record);
        }
        self.records
            .sort_by_key(|record| record.position.unwrap_or(u64::MAX));
    }

    pub(super) fn clear_preview(&mut self) {
        self.records.retain(|record| record.position.is_some());
        for record in &mut self.records {
            if let Body::Tool(tool) = &mut record.body {
                if tool.clear_live() {
                    record.rendered = RenderCache::default();
                }
            }
        }
    }

    pub(super) fn trim_front(&mut self) {
        if self.records.len() > MAX_RECORDS {
            self.records.drain(..self.records.len() - MAX_RECORDS);
            self.older = self.records.iter().find_map(|record| record.position);
        }
    }

    pub(super) fn trim_back(&mut self) -> Option<HistoryRequest> {
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

    pub(super) fn limit_live(&mut self) {
        let mut count = 0;
        self.records.retain(|record| {
            if record.position.is_some() {
                return true;
            }
            count += 1;
            count <= MAX_LIVE_RECORDS
        });
    }

    pub(super) fn record_or_insert(
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
            owner: None,
            position: None,
            body: body(),
            turn_summary: None,
            rendered: RenderCache::default(),
        });
        self.records.last_mut().expect("inserted record")
    }

    pub(super) fn tool_live(
        &mut self,
        key: Key,
        turn: Option<u64>,
        turn_number: Option<u64>,
        event: &Value,
    ) {
        let record = self.record_or_insert(key, turn, turn_number, || Body::Tool(Box::default()));
        if let Body::Tool(tool) = &mut record.body {
            if tool.apply_live(event) {
                record.rendered = RenderCache::default();
            }
        }
    }

    pub(super) fn row_count(&mut self, layout: Layout<'_>) -> usize {
        self.blocks(layout).map(|block| block.height()).sum()
    }

    pub(super) fn position_of(&mut self, anchor: &Anchor, layout: Layout<'_>) -> usize {
        let mut start = 0;
        let mut group_row = None;
        for block in self.blocks(layout) {
            if let Some(group) = &block.group {
                group_row = Some((
                    start + usize::from(block.heading.is_some()),
                    group.rows.len(),
                ));
            } else if !block.grouped {
                group_row = None;
            }
            if block.key == &anchor.key {
                return match anchor.line {
                    AnchorLine::Turn => start,
                    AnchorLine::Group(row) => group_row.map_or(start, |(start, height)| {
                        start + row.min(height.saturating_sub(1))
                    }),
                    AnchorLine::Body(_) if block.collapsed => {
                        group_row.map_or(start, |(first, _)| first)
                    }
                    AnchorLine::Body(line) => {
                        start + block.prefix_height() + line.min(block.rows.len().saturating_sub(1))
                    }
                };
            }
            start += block.height();
        }
        0
    }

    pub(super) fn anchor_at(&mut self, mut row: usize, layout: Layout<'_>) -> Option<Anchor> {
        for block in self.blocks(layout) {
            if row < block.height() {
                let line = if row == 0 && block.heading.is_some() {
                    AnchorLine::Turn
                } else if row < block.prefix_height() {
                    AnchorLine::Group(row - usize::from(block.heading.is_some()))
                } else {
                    AnchorLine::Body(row - block.prefix_height())
                };
                return Some(Anchor {
                    key: block.key.clone(),
                    line,
                });
            }
            row -= block.height();
        }
        None
    }

    pub(super) fn blocks<'a>(&'a mut self, layout: Layout<'a>) -> impl Iterator<Item = Block<'a>> {
        let mut spans = groups(&self.records)
            .into_iter()
            .map(|span| {
                let open = layout.items.group_open(&self.records[span.range.clone()]);
                (span.range, Some(span.summary), open)
            })
            .collect::<Vec<_>>()
            .into_iter();
        let mut current = spans.next();
        let mut records = self
            .records
            .iter_mut()
            .enumerate()
            .filter(|(_, record)| record.visible())
            .peekable();
        let from_start = self.older.is_none();
        let mut previous_turn = None;
        std::iter::from_fn(move || {
            let (index, record) = records.next()?;
            let heading = record
                .turn
                .filter(|&turn| {
                    Some(turn) != previous_turn && (previous_turn.is_some() || from_start)
                })
                .map(|_| {
                    record
                        .turn_number
                        .filter(|&number| number > 0)
                        .map_or(TurnHeading::Unnumbered, TurnHeading::Numbered)
                });
            previous_turn = record.turn.or(previous_turn);
            if current
                .as_ref()
                .is_some_and(|(range, _, _)| range.end <= index)
            {
                current = spans.next();
            }
            let member = current
                .as_mut()
                .filter(|(range, _, _)| range.contains(&index));
            let grouped = member.is_some();
            let collapsed = member.as_ref().is_some_and(|(_, _, open)| !open);
            let group = member.and_then(|(_, title, open)| {
                title
                    .take()
                    .map(|summary| GroupHeading::new(summary, *open, layout.width))
            });
            let (key, rows, detail) = if collapsed {
                (&record.key, &[][..], None)
            } else {
                let trim = records.peek().is_some_and(|(_, next)| {
                    record.turn == next.turn
                        && (matches!(next.body, Body::TurnSummary)
                            || record.tool().is_some() && next.tool().is_some())
                });
                let mode = if grouped {
                    ToolDetails::GroupMember
                } else {
                    layout.tools
                };
                let (key, rows, detail) = record.render(Layout {
                    tools: mode,
                    ..layout
                });
                (
                    key,
                    if trim && rows.last().is_some_and(|row| row.line.spans.is_empty()) {
                        &rows[..rows.len() - 1]
                    } else {
                        rows
                    },
                    detail,
                )
            };
            Some(Block {
                key,
                heading,
                group,
                grouped,
                collapsed,
                rows,
                detail,
            })
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct Layout<'a> {
    pub(super) width: usize,
    pub(super) tools: ToolDetails,
    pub(super) assistant: AssistantView,
    pub(super) agents: &'a AgentCards,
    pub(super) items: &'a Details,
}

pub(super) struct Block<'a> {
    pub(super) key: &'a Key,
    pub(super) heading: Option<TurnHeading>,
    pub(super) group: Option<GroupHeading>,
    pub(super) rows: &'a [Row],
    pub(super) detail: Option<DetailRecord<'a>>,
    grouped: bool,
    collapsed: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TurnHeading {
    Numbered(u64),
    Unnumbered,
}

impl Block<'_> {
    pub(super) fn height(&self) -> usize {
        self.rows.len() + self.prefix_height()
    }

    fn prefix_height(&self) -> usize {
        usize::from(self.heading.is_some())
            + self.group.as_ref().map_or(0, |group| group.rows.len())
    }
}
