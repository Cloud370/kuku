use kuku_client::{HistoryPage, RetryPhase, RetrySource, RetryStopReason, RewindGroup};
use serde_json::Value;

use crate::theme::CardFrame;

use super::agent::AgentCards;
use super::details::{compact_file_rows, groups, DetailRecord, Details, GroupHeading};
use super::{
    Anchor, AnchorLine, AssistantView, Body, CardTiming, Key, Record, RewindGroupState, Row,
    ToolDetails, MAX_BODY_BYTES, MAX_LIVE_RECORDS, MAX_RECORDS,
};

#[derive(Debug, Default)]
pub(super) struct RenderCache {
    pub(super) preview: Option<(super::PreviewLayout, Vec<Row>)>,
    pub(super) source: Option<(usize, Vec<Row>)>,
    pub(super) streaming: Option<Box<super::stream::StreamingReply>>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Window {
    pub(super) records: Vec<Record>,
    pub(super) rewind_groups: Vec<RewindGroupState>,
    pub(super) older: Option<u64>,
}

struct ActiveGroup {
    range: std::ops::Range<usize>,
    heading: Option<GroupHeading>,
    open: bool,
    framed: bool,
    preview: Option<Vec<usize>>,
    remaining: usize,
    timing: CardTiming,
}

impl Window {
    pub(super) fn from_page(page: HistoryPage) -> Self {
        let mut window = Self {
            older: page.next_before,
            ..Self::default()
        };
        window.merge(page);
        window
    }

    pub(super) fn merge(&mut self, page: HistoryPage) {
        for record in page.entries.into_iter().filter_map(Record::from_entry) {
            self.insert_record(record);
        }
        self.sort_records();
        self.reconcile_retry_errors();
        self.add_rewind_groups(page.rewind_groups);
    }

    pub(super) fn last_position(&self) -> Option<u64> {
        self.records
            .iter()
            .filter_map(|record| record.position)
            .chain(self.rewind_groups.iter().map(RewindGroupState::position))
            .max()
    }

    pub(super) fn contains(&self, key: &Key) -> bool {
        self.records.iter().any(|record| &record.key == key)
            || self.rewind_groups.iter().any(|group| &group.key == key)
    }

    pub(super) fn add_rewind_groups(&mut self, groups: impl IntoIterator<Item = RewindGroup>) {
        for summary in groups {
            if let Some(group) = self
                .rewind_groups
                .iter_mut()
                .find(|group| group.summary.rewind_event_id == summary.rewind_event_id)
            {
                group.update_summary(summary);
            } else {
                self.rewind_groups.push(RewindGroupState::new(summary));
            }
        }
        self.rewind_groups.sort_by_key(RewindGroupState::position);
    }

    pub(super) fn positions(&self) -> Vec<(u64, Key)> {
        let mut positions = self
            .records
            .iter()
            .filter_map(|record| {
                record
                    .position
                    .map(|position| (position, record.key.clone()))
            })
            .chain(
                self.rewind_groups
                    .iter()
                    .map(|group| (group.position(), group.key.clone())),
            )
            .collect::<Vec<_>>();
        positions.sort_by_key(|(position, _)| *position);
        positions
    }

    pub(super) fn remove(&mut self, key: &Key) {
        self.records.retain(|record| &record.key != key);
        self.reconcile_retry_errors();
    }

    pub(super) fn insert(&mut self, record: Record) {
        self.insert_record(record);
        self.sort_records();
        self.reconcile_retry_errors();
    }

    fn insert_record(&mut self, record: Record) {
        if let Some(previous) = self.records.iter_mut().find(|old| old.key == record.key) {
            previous.merge(record);
        } else {
            self.records.push(record);
        }
    }

    pub(super) fn position_preview_before(&mut self, key: &Key, event: u64, turn: Option<u64>) {
        let Some(record) = self.records.iter_mut().find(|record| &record.key == key) else {
            return;
        };
        if record.position.is_none()
            && record.turn == turn
            && matches!(&record.body, Body::Assistant(text) if !text.trim().is_empty())
        {
            record.before_event = Some(event);
            self.sort_records();
        }
    }

    pub(super) fn stopped_retry_preview(
        &self,
        key: &Key,
        turn: Option<u64>,
    ) -> Option<(String, Option<u64>)> {
        let turn = turn?;
        let request_ids = self
            .records
            .iter()
            .filter_map(|record| match &record.body {
                Body::Retry { metadata, .. }
                    if record.turn == Some(turn)
                        && metadata.source == RetrySource::Provider
                        && metadata.phase == RetryPhase::Stopped
                        && metadata.stop_reason == Some(RetryStopReason::StreamStarted)
                        && crate::retry_notice::decision(metadata) == Some(false) =>
                {
                    metadata.request_id.clone()
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let has_correlated_error = request_ids.into_iter().any(|request_id| {
            self.records.iter().any(|record| {
                record.turn == Some(turn)
                    && matches!(
                        &record.body,
                        Body::ModelError {
                            request_id: Some(error_id),
                            ..
                        } if error_id == &request_id
                    )
            })
        });
        if !has_correlated_error {
            return None;
        }
        self.records
            .iter()
            .find(|record| {
                &record.key == key
                    && record.turn == Some(turn)
                    && record.position.is_none()
                    && matches!(&record.body, Body::Assistant(text) if !text.is_empty())
            })
            .and_then(|record| match &record.body {
                Body::Assistant(text) => Some((text.clone(), record.before_event)),
                _ => None,
            })
    }

    pub(super) fn preserve_interrupted_reply(
        &self,
        record: &mut Option<Record>,
        execution: Option<u64>,
        turn: Option<u64>,
    ) {
        let Some(execution) = execution else {
            return;
        };
        let Some((text, before_event)) = self.stopped_retry_preview(&Key::Preview(execution), turn)
        else {
            return;
        };
        let Some(record) = record else { return };
        if let Body::Interrupted(interrupted) = &mut record.body {
            if interrupted.is_empty() || interrupted == super::DISPLAY_LIMIT_NOTICE {
                let truncated_notice = interrupted == super::DISPLAY_LIMIT_NOTICE;
                *interrupted = text;
                if truncated_notice && !interrupted.ends_with(super::DISPLAY_LIMIT_NOTICE) {
                    interrupted.push_str(super::DISPLAY_LIMIT_NOTICE);
                }
            }
            record.before_event = before_event;
        }
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
        self.trim_front_to(MAX_RECORDS);
    }

    pub(super) fn trim_front_to(&mut self, limit: usize) {
        let positions = self.positions();
        let live = self
            .records
            .iter()
            .filter(|record| record.position.is_none())
            .count();
        let keep = limit.saturating_sub(live);
        if positions.len() > keep {
            let first = positions[positions.len() - keep].0;
            self.records
                .retain(|record| record.position.is_none_or(|position| position >= first));
            self.rewind_groups.retain(|group| group.position() >= first);
            self.older = Some(first);
        }
        self.reconcile_retry_errors();
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
        let execution_id = match &key {
            Key::Preview(execution_id) => Some(*execution_id),
            Key::Event(_) | Key::Compaction(_) | Key::RewindGroup(_) | Key::Tool { .. } => None,
        };
        if let Some(index) = self.records.iter().position(|record| record.key == key) {
            let record = &mut self.records[index];
            record.turn = record.turn.or(turn);
            record.turn_number = record.turn_number.or(turn_number);
            record.execution_id = record.execution_id.or(execution_id);
            return record;
        }
        self.records.push(Record {
            key,
            turn,
            turn_number,
            owner: None,
            timing: CardTiming::default(),
            execution_id,
            position: None,
            before_event: None,
            body: body(),
            suppress_interruption: false,
            turn_summary: None,
            reasoning_preview: String::new(),
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
        Block::row_count(&self.blocks(layout))
    }

    pub(super) fn position_of(&mut self, anchor: &Anchor, layout: Layout<'_>) -> usize {
        Block::position_of(&self.blocks(layout), anchor)
    }

    pub(super) fn anchor_at(&mut self, row: usize, layout: Layout<'_>) -> Option<Anchor> {
        Block::anchor_at(&self.blocks(layout), row)
    }

    pub(super) fn blocks<'a>(&'a mut self, layout: Layout<'a>) -> Vec<Block<'a>> {
        let from_start = self.older.is_none();
        let group_frame = CardFrame::timeline(layout.width);
        let Self {
            records: window_records,
            rewind_groups,
            ..
        } = self;
        let compact_files = compact_file_rows(window_records);
        let mut spans = groups(window_records)
            .into_iter()
            .map(|span| {
                let open = layout.items.group_open(&window_records[span.range.clone()]);
                let range = span.range.clone();
                let framed = span.summary.framed();
                let preview = span.summary.activity_preview().map(<[usize]>::to_vec);
                let remaining = span.summary.activity_remaining();
                let timing = CardTiming::group(&window_records[range.clone()]);
                let heading = GroupHeading::from_span(
                    span,
                    window_records,
                    open,
                    layout.width,
                    layout.agents,
                );
                let remaining = if open { 0 } else { remaining };
                ActiveGroup {
                    range,
                    heading: Some(heading),
                    open,
                    framed,
                    preview,
                    remaining,
                    timing,
                }
            })
            .collect::<Vec<_>>()
            .into_iter();
        let mut current = spans.next();
        let mut records = super::turn_summary::records_in_display_order(window_records)
            .into_iter()
            .peekable();
        let mut previous_turn = None;
        let mut retry_turn = None;
        let mut follows_reply = false;
        let mut blocks = std::iter::from_fn(move || {
            let (index, position, record) = records.next()?;
            let footer_summary = record.turn_summary.as_ref().filter(|_| follows_reply);
            let timing = if record.turn_summary.is_some() {
                CardTiming::default()
            } else {
                record.timing
            };
            follows_reply = group_frame.bordered()
                && record.has_reply()
                && record.turn_summary.is_none()
                && records.peek().is_some_and(|(_, _, next)| {
                    record.turn.is_some()
                        && next.turn == record.turn
                        && matches!(next.body, Body::TurnSummary)
                        && next.turn_summary.is_some()
                });
            let retry = matches!(&record.body, Body::Retry { .. });
            let retry_heading = retry && retry_turn != Some(record.turn);
            let retry_end = retry
                && !records.peek().is_some_and(|(_, _, next)| {
                    next.turn == record.turn && matches!(&next.body, Body::Retry { .. })
                });
            retry_turn = retry.then_some(record.turn);
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
            let gap = (heading.is_some() && previous_turn.is_some()).then_some(BlockGap::Turn);
            previous_turn = record.turn.or(previous_turn);
            if current
                .as_ref()
                .is_some_and(|group| group.range.end <= index)
            {
                current = spans.next();
            }
            let mut member = current
                .as_mut()
                .filter(|group| group.range.contains(&index));
            let grouped = member.is_some();
            let framed = member.as_ref().is_some_and(|group| group.framed);
            let preview = member
                .as_ref()
                .filter(|group| !group.open)
                .and_then(|group| {
                    group
                        .preview
                        .as_ref()
                        .map(|entries| entries[index - group.range.start])
                })
                .unwrap_or(0);
            let collapsed = member
                .as_ref()
                .is_some_and(|group| !group.open && preview == 0);
            let closes_group = member
                .as_ref()
                .is_some_and(|group| index + 1 == group.range.end);
            let remaining = member.as_ref().map_or(0, |group| group.remaining);
            let group = member.as_mut().and_then(|group| group.heading.take());
            let timing = member.map_or(timing, |member| {
                if closes_group || (member.framed && !member.open && group.is_some()) {
                    member.timing
                } else {
                    CardTiming::default()
                }
            });
            let user_turn = heading.filter(|_| {
                layout.width >= 14 && matches!(&record.body, Body::User { sender: None, .. })
            });
            let heading = heading.filter(|_| user_turn.is_none());
            let thinking = matches!(record.key, Key::Preview(_))
                && matches!(&record.body, Body::Assistant(text) if text.trim().is_empty());
            let footer = if let Some(summary) = footer_summary {
                super::turn_summary::footer(summary, record.turn_number, &group_frame)
            } else if closes_group && (!framed || !collapsed) {
                super::details::activity_footer(&group_frame, remaining)
            } else {
                Vec::new()
            };
            let (key, rows, detail) = if collapsed || footer_summary.is_some() {
                (&record.key, &[][..], None)
            } else {
                let mode = if compact_files[index]
                    || grouped && record.tool().is_some_and(super::Tool::compact_file_change)
                {
                    ToolDetails::CompactFile
                } else if preview > 0 {
                    ToolDetails::ActivityPreview(preview)
                } else if grouped {
                    ToolDetails::GroupMember
                } else {
                    layout.tools
                };
                let (key, rows, detail) = record.render(
                    Layout {
                        width: if grouped {
                            group_frame.content_width()
                        } else {
                            layout.width
                        },
                        tools: mode,
                        ..layout
                    },
                    retry_heading,
                    retry_end,
                    user_turn,
                    grouped,
                );
                (
                    key,
                    if follows_reply {
                        &rows[..rows.len().saturating_sub(2)]
                    } else if grouped && rows.last().is_some_and(|row| row.line.spans.is_empty()) {
                        &rows[..rows.len() - 1]
                    } else {
                        rows
                    },
                    detail,
                )
            };
            let note = match detail {
                Some(DetailRecord::Reasoning(_)) if !rows.is_empty() => {
                    layout.notes.map_or(&[][..], |note| note.rows_for(key))
                }
                _ => &[],
            };
            Some(Block {
                key,
                position,
                heading,
                turn: previous_turn,
                gap,
                timing,
                thinking,
                group,
                grouped,
                collapsed,
                rows,
                note,
                footer,
                detail,
                rewind_group: None,
            })
        })
        .collect::<Vec<_>>();
        for group in rewind_groups {
            group.render(layout.width);
            blocks.push(Block {
                key: &group.key,
                position: group.position(),
                heading: None,
                turn: None,
                gap: None,
                timing: CardTiming::default(),
                thinking: false,
                group: None,
                grouped: false,
                collapsed: false,
                rows: &group.rows,
                note: &[],
                footer: Vec::new(),
                detail: None,
                rewind_group: Some(group),
            });
        }
        blocks.sort_by_key(|block| block.position);
        super::timing::space_timed_cards(&mut blocks, layout.width);
        blocks
    }

    fn sort_records(&mut self) {
        self.records
            .sort_by_key(|record| match (record.before_event, record.position) {
                (Some(event), _) => (event, 0),
                (None, Some(position)) => (position, 1),
                (None, None) => (u64::MAX, 2),
            });
    }

    pub(super) fn reconcile_retry_errors(&mut self) {
        let retry_messages = self
            .records
            .iter()
            .filter_map(|record| match &record.body {
                Body::Retry { metadata, .. }
                    if metadata.source == RetrySource::Provider
                        && crate::retry_notice::decision(metadata) == Some(false) =>
                {
                    let agent = record.owner.as_ref()?;
                    let turn = record.turn?;
                    let request_id = metadata.request_id.as_ref()?;
                    Some((
                        agent.clone(),
                        turn,
                        request_id.clone(),
                        record.execution_id,
                        record.position?,
                        metadata
                            .message
                            .as_deref()
                            .filter(|message| !message.trim().is_empty())
                            .map(|message| super::capped(Some(message), MAX_BODY_BYTES)),
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let stopped_provider_retries = self
            .records
            .iter()
            .filter_map(|record| match &record.body {
                Body::Retry { metadata, .. }
                    if metadata.source == RetrySource::Provider
                        && crate::retry_notice::decision(metadata) == Some(false) =>
                {
                    Some((
                        record.owner.as_ref()?.clone(),
                        record.turn?,
                        metadata.request_id.as_ref()?.clone(),
                        record.execution_id,
                        record.position?,
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let error_scopes = self
            .records
            .iter()
            .filter_map(|record| match &record.body {
                Body::ModelError { .. } => Some((
                    record.owner.as_ref()?.clone(),
                    record.turn?,
                    record.execution_id,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        let error_requests = self
            .records
            .iter()
            .filter_map(|record| match &record.body {
                Body::ModelError {
                    request_id: Some(request_id),
                    ..
                } if !request_id.trim().is_empty() => Some((
                    record.owner.as_ref()?.clone(),
                    record.turn?,
                    request_id.clone(),
                    record.execution_id,
                    record.position?,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        let correlated_errors = error_requests
            .iter()
            .filter(|(agent, turn, request_id, execution_id, error_position)| {
                let matching_retry = stopped_provider_retries.iter().any(
                    |(retry_agent, retry_turn, retry_id, retry_execution, retry_position)| {
                        agent == retry_agent
                            && turn == retry_turn
                            && request_id == retry_id
                            && execution_matches(*execution_id, *retry_execution)
                            && retry_position < error_position
                    },
                );
                let legacy_error = error_scopes
                    .iter()
                    .filter(|(error_agent, error_turn, error_execution)| {
                        agent == error_agent
                            && turn == error_turn
                            && execution_matches(*execution_id, *error_execution)
                    })
                    .count()
                    == 1;
                matching_retry || legacy_error
            })
            .cloned()
            .collect::<Vec<_>>();
        let interruptions = self
            .records
            .iter()
            .filter_map(|record| match &record.body {
                Body::Notice("Execution interrupted", ..) => Some((
                    record.owner.as_ref()?.clone(),
                    record.turn?,
                    record.execution_id,
                    record.position?,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        for record in &mut self.records {
            let mut changed = false;
            let owner = record.owner.clone();
            let turn = record.turn;
            match &mut record.body {
                Body::ModelError {
                    request_id: Some(request_id),
                    diagnostic,
                    hint,
                    fallback_hint,
                    retry_message,
                    reply_interrupted,
                    ..
                } => {
                    let message = retry_messages
                        .iter()
                        .rev()
                        .find(
                            |(agent, retry_turn, retry_id, retry_execution, retry_position, _)| {
                                owner.as_ref() == Some(agent)
                                    && turn == Some(*retry_turn)
                                    && retry_id == request_id
                                    && execution_matches(record.execution_id, *retry_execution)
                                    && record
                                        .position
                                        .is_some_and(|position| *retry_position < position)
                            },
                        )
                        .and_then(|(_, _, _, _, _, message)| message.as_ref());
                    let next_retry_message = message.cloned();
                    if retry_message != &next_retry_message {
                        *retry_message = next_retry_message;
                        changed = true;
                    }
                    let next_hint = match message {
                        Some(message) if message.as_str() == diagnostic.as_str() => String::new(),
                        Some(message) => message.clone(),
                        None => fallback_hint.clone(),
                    };
                    if hint.as_str() != next_hint {
                        *hint = next_hint;
                        changed = true;
                    }
                    let next_reply_interrupted = correlated_errors.iter().any(
                        |(agent, error_turn, error_id, error_execution, error_position)| {
                            owner.as_ref() == Some(agent)
                                && turn == Some(*error_turn)
                                && request_id == error_id
                                && execution_matches(record.execution_id, *error_execution)
                                && record.position == Some(*error_position)
                                && interruptions.iter().any(
                                    |(
                                        interruption_agent,
                                        interruption_turn,
                                        interruption_execution,
                                        interruption_position,
                                    )| {
                                        agent == interruption_agent
                                            && error_turn == interruption_turn
                                            && execution_matches(
                                                *error_execution,
                                                *interruption_execution,
                                            )
                                            && error_position < interruption_position
                                    },
                                )
                        },
                    );
                    if *reply_interrupted != next_reply_interrupted {
                        *reply_interrupted = next_reply_interrupted;
                        changed = true;
                    }
                }
                Body::Notice("Execution interrupted", ..) => {
                    let next_suppressed = correlated_errors.iter().any(
                        |(agent, error_turn, _, error_execution, error_position)| {
                            owner.as_ref() == Some(agent)
                                && turn == Some(*error_turn)
                                && execution_matches(record.execution_id, *error_execution)
                                && record
                                    .position
                                    .is_some_and(|position| *error_position < position)
                        },
                    );
                    if record.suppress_interruption != next_suppressed {
                        record.suppress_interruption = next_suppressed;
                        changed = true;
                    }
                }
                Body::Retry {
                    metadata,
                    include_detail,
                } => {
                    let matched_error = crate::retry_notice::decision(metadata) == Some(false)
                        && metadata.request_id.as_ref().is_some_and(|request_id| {
                            error_requests.iter().any(
                                |(agent, error_turn, error_id, error_execution, error_position)| {
                                    owner.as_ref() == Some(agent)
                                        && turn == Some(*error_turn)
                                        && error_id == request_id
                                        && execution_matches(record.execution_id, *error_execution)
                                        && record
                                            .position
                                            .is_some_and(|position| position < *error_position)
                                },
                            )
                        });
                    let next_include_detail = !matched_error;
                    if *include_detail != next_include_detail {
                        *include_detail = next_include_detail;
                        changed = true;
                    }
                }
                _ => {}
            }
            if changed {
                record.rendered = RenderCache::default();
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Layout<'a> {
    pub(super) width: usize,
    pub(super) tools: ToolDetails,
    pub(super) assistant: AssistantView,
    pub(super) agents: &'a AgentCards,
    pub(super) items: &'a Details,
    pub(super) notes: Option<&'a super::notes::ThinkingRead>,
}

pub(super) struct Block<'a> {
    pub(super) key: &'a Key,
    pub(super) position: u64,
    pub(super) heading: Option<TurnHeading>,
    pub(super) turn: Option<u64>,
    pub(super) gap: Option<BlockGap>,
    pub(super) timing: CardTiming,
    pub(super) thinking: bool,
    pub(super) group: Option<GroupHeading>,
    pub(super) rows: &'a [Row],
    note: &'a [Row],
    pub(super) footer: Vec<Row>,
    pub(super) detail: Option<DetailRecord<'a>>,
    pub(super) rewind_group: Option<&'a RewindGroupState>,
    pub(super) grouped: bool,
    pub(super) collapsed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockGap {
    Turn,
    Card,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnHeading {
    Numbered(u64),
    Unnumbered,
}

impl Block<'_> {
    pub(super) fn row_count(blocks: &[Self]) -> usize {
        blocks.iter().map(Self::height).sum()
    }

    pub(super) fn position_of(blocks: &[Self], anchor: &Anchor) -> usize {
        let mut start = 0;
        let mut group_row = None;
        for block in blocks {
            if let Some(group) = &block.group {
                group_row = Some((
                    start + usize::from(block.gap.is_some()) + usize::from(block.heading.is_some()),
                    group.rows.len(),
                ));
            } else if !block.grouped {
                group_row = None;
            }
            if block.key == &anchor.key {
                return match anchor.line {
                    AnchorLine::Gap => start,
                    AnchorLine::Turn => start + usize::from(block.gap.is_some()),
                    AnchorLine::RewindEntry { event_id, line } => block
                        .rewind_group
                        .and_then(|group| group.row_for_entry(event_id, line))
                        .map_or(start, |row| start + row),
                    AnchorLine::Group(row) if block.rewind_group.is_some() => {
                        start + row.min(block.rows.len().saturating_sub(1))
                    }
                    AnchorLine::Group(row) => group_row.map_or(start, |(start, height)| {
                        start + row.min(height.saturating_sub(1))
                    }),
                    AnchorLine::Body(_) | AnchorLine::Note(_) if block.collapsed => {
                        group_row.map_or(start, |(first, _)| first)
                    }
                    AnchorLine::Body(line) => {
                        let line =
                            line.min((block.rows.len() + block.footer.len()).saturating_sub(1));
                        start
                            + block.prefix_height()
                            + line
                            + if line == 0 { 0 } else { block.note_height() }
                    }
                    AnchorLine::Script(detail) => {
                        start
                            + block.prefix_height()
                            + block
                                .rows
                                .iter()
                                .position(|row| row.script_detail == Some(detail))
                                .unwrap_or_default()
                    }
                    AnchorLine::InlineOutput(target) => {
                        start
                            + block.prefix_height()
                            + block
                                .rows
                                .iter()
                                .position(|row| {
                                    row.inline_output.is_some_and(|(source, action)| {
                                        target == source && action.is_toggle()
                                    })
                                })
                                .unwrap_or_default()
                    }
                    AnchorLine::CommandArguments => {
                        start
                            + block.prefix_height()
                            + block
                                .rows
                                .iter()
                                .position(|row| row.command_arguments.is_some())
                                .unwrap_or_default()
                    }
                    AnchorLine::Note(line) => {
                        start + block.prefix_height() + line.min(block.note_height())
                    }
                    AnchorLine::Footer(_) if block.collapsed && block.footer.is_empty() => {
                        group_row.map_or(start, |(first, _)| first)
                    }
                    AnchorLine::Footer(line) => {
                        let footer = block.prefix_height() + block.body_height();
                        let offset = footer + line.min(block.footer.len().saturating_sub(1));
                        start + offset.min(block.height().saturating_sub(1))
                    }
                };
            }
            start += block.height();
        }
        0
    }

    pub(super) fn anchor_at(blocks: &[Self], mut row: usize) -> Option<Anchor> {
        for block in blocks {
            if row < block.height() {
                let heading_row = usize::from(block.gap.is_some());
                let prefix = block.prefix_height();
                let line = if row == 0 && block.gap.is_some() {
                    AnchorLine::Gap
                } else if row == heading_row && block.heading.is_some() {
                    AnchorLine::Turn
                } else if let Some(group) = block.rewind_group {
                    group
                        .entry_at(row)
                        .map_or(AnchorLine::Group(row), |(event_id, line)| {
                            AnchorLine::RewindEntry { event_id, line }
                        })
                } else if row < prefix {
                    AnchorLine::Group(row.saturating_sub(
                        usize::from(block.gap.is_some()) + usize::from(block.heading.is_some()),
                    ))
                } else if row >= prefix + block.body_height() {
                    AnchorLine::Footer(row - prefix - block.body_height())
                } else {
                    let body = row - prefix;
                    let notes = block.note_height();
                    if body > 0 && body <= notes {
                        AnchorLine::Note(body)
                    } else {
                        AnchorLine::Body(body.saturating_sub(notes))
                    }
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

    pub(super) fn height(&self) -> usize {
        self.body_height() + self.prefix_height() + self.footer.len()
    }

    pub(super) fn body_height(&self) -> usize {
        self.rows.len() + self.note_height()
    }

    pub(super) fn note_height(&self) -> usize {
        self.note.len().saturating_sub(1)
    }

    pub(super) fn body_rows(&self) -> impl DoubleEndedIterator<Item = &Row> {
        self.note
            .iter()
            .chain(self.rows.iter().skip(usize::from(!self.note.is_empty())))
    }

    fn prefix_height(&self) -> usize {
        usize::from(self.gap.is_some())
            + usize::from(self.heading.is_some())
            + self.group.as_ref().map_or(0, |group| group.rows.len())
    }
}

fn execution_matches(left: Option<u64>, right: Option<u64>) -> bool {
    left.is_none() || right.is_none() || left == right
}
