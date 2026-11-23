use std::time::Duration;

use kuku_client::{HistoryEntry, HistoryKind};
use ratatui::text::{Line, Span};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

use crate::text::format_elapsed;
use crate::theme::{CardFrame, TranscriptTone};

use super::window::BlockGap;
use super::{body_inset, Block, Record, Row};

pub(super) fn connector_column(frame: &CardFrame) -> Option<usize> {
    frame
        .bordered()
        .then(|| frame.content_inset() + usize::from(body_inset(frame.content_width())))
}

pub(super) fn space_timed_cards(blocks: &mut [Block<'_>], width: usize) {
    let inset = connector_column(&CardFrame::timeline(width)).map_or(1, |column| column + 2);
    let mut previous_timed_turn = None;
    for block in blocks.iter_mut().filter(|block| block.height() != 0) {
        if previous_timed_turn.is_some() && previous_timed_turn == block.turn && block.gap.is_none()
        {
            block.gap = Some(BlockGap::Card);
        }
        let trailing = block
            .footer
            .last()
            .or_else(|| block.body_rows().next_back())
            .or_else(|| block.group.as_ref()?.rows.last());
        previous_timed_turn = block.turn.filter(|_| {
            trailing.is_some_and(|row| row.line.spans.is_empty()) && block.timing.fits(width, inset)
        });
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CardTiming {
    start: Option<OffsetDateTime>,
    end: Option<OffsetDateTime>,
    measured_ms: Option<u64>,
    turn_started_at_ms: Option<u64>,
}

impl CardTiming {
    pub(super) fn from_entry(entry: &HistoryEntry) -> Self {
        let timestamp = entry
            .details
            .get("ts")
            .and_then(Value::as_str)
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok());
        let starting = entry.kind == HistoryKind::ToolCall
            || entry.kind == HistoryKind::Compaction
                && entry.details.get("status").and_then(Value::as_str) == Some("running");
        let measured_ms = match entry.kind {
            HistoryKind::ToolResult => entry
                .details
                .get("structured")
                .filter(|facts| facts["kind"] == "command_result")
                .and_then(|facts| facts["duration_ms"].as_u64()),
            HistoryKind::Compaction => entry.details.get("elapsed_ms").and_then(Value::as_u64),
            _ => None,
        };
        let (start, end) = if starting {
            (timestamp, None)
        } else {
            (None, timestamp)
        };
        Self {
            start,
            end,
            measured_ms,
            turn_started_at_ms: entry
                .details
                .get("turn_started_at_ms")
                .and_then(Value::as_u64),
        }
    }

    pub(super) fn merge(&mut self, incoming: Self) {
        self.start = self.start.or(incoming.start);
        self.end = self.end.max(incoming.end);
        self.measured_ms = self.measured_ms.or(incoming.measured_ms);
        self.turn_started_at_ms = self.turn_started_at_ms.or(incoming.turn_started_at_ms);
    }

    pub(super) fn group(records: &[Record]) -> Self {
        let mut timing = Self::default();
        let mut complete = true;
        for record in records {
            let member = record.timing;
            timing.start = timing.start.into_iter().chain(member.start).min();
            timing.end = timing.end.max(member.end.or(member.start));
            timing.turn_started_at_ms = timing.turn_started_at_ms.or(member.turn_started_at_ms);
            complete &= member
                .start
                .zip(member.end)
                .is_some_and(|(start, end)| start <= end);
        }
        if !complete {
            timing.start = None;
        }
        timing
    }

    fn elapsed_ms(self) -> Option<u64> {
        self.measured_ms.or_else(|| {
            let elapsed = self.end? - self.start?;
            if elapsed.is_negative() {
                return None;
            }
            u64::try_from(elapsed.whole_milliseconds()).ok()
        })
    }

    fn fits(self, width: usize, inset: usize) -> bool {
        if self.end.or(self.start).is_some() {
            width.saturating_sub(inset) >= "00:00:00Z".len()
        } else {
            self.measured_ms.is_some() && self.line(width, inset).is_some()
        }
    }

    pub(super) fn line(self, width: usize, inset: usize) -> Option<Line<'static>> {
        let timestamp = self.end.or(self.start);
        let mut line = timestamp.map_or_else(Line::default, |timestamp| {
            let stamp = timestamp.to_offset(UtcOffset::UTC);
            Line::styled(
                format!(
                    "{:02}:{:02}:{:02}Z",
                    stamp.hour(),
                    stamp.minute(),
                    stamp.second()
                ),
                TranscriptTone::Label.style(),
            )
        });
        let duration = self.elapsed_ms().map(|ms| {
            let (value, unit) = match ms {
                0..=999 => (ms.to_string(), "ms"),
                1_000..=59_999 => (format!("{:.1}", ms as f64 / 1_000.0), "s"),
                _ => (format_elapsed(Duration::from_millis(ms)), ""),
            };
            let separator = if timestamp.is_some() { " · " } else { "" };
            Row::metric(separator, value, unit, TranscriptTone::Value)
        });
        let turn_elapsed = self
            .turn_started_at_ms
            .zip(timestamp)
            .and_then(|(start, stamp)| {
                u64::try_from(stamp.unix_timestamp_nanos() / 1_000_000)
                    .ok()?
                    .checked_sub(start)
            });
        if let Some(ms) = turn_elapsed {
            let offset = Row::metric(
                " · T+",
                format_elapsed(Duration::from_millis(ms)),
                "",
                TranscriptTone::Value,
            );
            let duration_width = duration.as_ref().map_or(0, Line::width);
            if inset + line.width() + offset.width() + duration_width <= width {
                line.extend(offset.spans);
            }
        }
        if let Some(duration) = duration {
            if inset + line.width() + duration.width() <= width {
                line.extend(duration.spans);
            }
        }
        if line.spans.is_empty() || inset + line.width() > width {
            return None;
        }
        line.spans.insert(0, Span::raw(" ".repeat(inset)));
        Some(line)
    }
}
