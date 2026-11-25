use std::time::Duration;

use kuku_client::{HistoryEntry, HistoryKind};
use ratatui::text::{Line, Span};
use serde_json::Value;
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::text::{format_duration_ms, format_elapsed};
use crate::theme::{CardFrame, TranscriptTone};

use super::window::BlockGap;
use super::{Block, Record, Row, body_inset};

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
        if let Some(timestamp) = self.end.or(self.start) {
            let available = width.saturating_sub(inset);
            available >= 9 || (available == 8 && client_stamp(timestamp).1.is_empty())
        } else {
            self.measured_ms.is_some() && self.line(width, inset).is_some()
        }
    }

    pub(super) fn line(self, width: usize, inset: usize) -> Option<Line<'static>> {
        let timestamp = self.end.or(self.start);
        let mut line = timestamp.map_or_else(Line::default, |timestamp| {
            let (stamp, zone) = client_stamp(timestamp);
            Line::styled(
                format!(
                    "{:02}:{:02}:{:02}{zone}",
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
                1_000..=59_949 => (format!("{:.1}", ms as f64 / 1_000.0), "s"),
                _ => (format_duration_ms(ms), ""),
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

/// Resolve the offset at the event instant (not at the time of rendering), so
/// historical cards use the client's timezone even across DST transitions.
/// If the platform cannot determine its offset, identify UTC honestly.
fn client_stamp(timestamp: OffsetDateTime) -> (OffsetDateTime, &'static str) {
    match UtcOffset::local_offset_at(timestamp)
        .ok()
        .and_then(|offset| timestamp.checked_to_offset(offset))
    {
        Some(local) => (local, ""),
        None => (timestamp.to_offset(UtcOffset::UTC), "Z"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    // Changing TZ in a running, multithreaded test process is not safe. Run
    // the same test in isolated processes with a fixed zone and fixture events.
    #[test]
    fn card_clock_uses_client_zone_at_event_instant() {
        const CASE: &str = "KUKU_TEST_CARD_CLOCK_ZONE";
        if let Ok(case) = std::env::var(CASE) {
            let expected = match case.as_str() {
                "china" => ["02:00:00", "02:00:00"],
                "eastern" => ["13:00:00", "14:00:00"],
                _ => panic!("unknown timezone fixture"),
            };
            for (source, clock) in ["2026-11-25T18:00:00Z", "2026-07-01T18:00:00Z"]
                .into_iter()
                .zip(expected)
            {
                let timestamp = OffsetDateTime::parse(source, &Rfc3339).unwrap();
                let timing = CardTiming {
                    start: Some(timestamp - time::Duration::seconds(2)),
                    end: Some(timestamp),
                    turn_started_at_ms: Some(
                        u64::try_from(timestamp.unix_timestamp()).unwrap() * 1000 - 3000,
                    ),
                    ..CardTiming::default()
                };
                assert_eq!(
                    timing.line(60, 0).unwrap().to_string(),
                    format!("{clock} · T+3s · 2.0s")
                );
                assert!(timing.fits(10, 2));
                assert_eq!(
                    timing.line(10, 2).unwrap().to_string(),
                    format!("  {clock}")
                );
                assert!(!timing.fits(9, 2));
                assert!(timing.line(9, 2).is_none());
            }
            return;
        }
        for (case, zone) in [
            ("china", "CST-8"),
            ("eastern", "EST5EDT,M3.2.0/2,M11.1.0/2"),
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "transcript::timing::tests::card_clock_uses_client_zone_at_event_instant",
                ])
                .env(CASE, case)
                .env("TZ", zone)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{case}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
    }
}
