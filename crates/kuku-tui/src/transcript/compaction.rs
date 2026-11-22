use kuku_client::HistoryEntry;
use ratatui::text::{Line, Span};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use crate::text::{compact_token_count, shorten, single_line};
use crate::theme::{CardFrame, TranscriptTone, ACCENT, MUTED, SUCCESS, WARNING};

use super::{capped, message, render_rows, Row, MAX_BODY_BYTES, MAX_VISUAL_ROWS};

#[derive(Debug, Clone)]
pub(super) enum Compaction {
    Queued,
    Running {
        started: Option<OffsetDateTime>,
        provider_limit: bool,
    },
    Completed {
        elapsed_ms: Option<u64>,
        input_tokens: Option<(u64, u64)>,
        summary_output_tokens: Option<u64>,
        messages: Option<u64>,
        summary: String,
    },
    Ended {
        elapsed_ms: Option<u64>,
        outcome: End,
        reason: String,
        continues: bool,
    },
    Restored,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum End {
    Failed,
    Cancelled,
    Interrupted,
}

impl Compaction {
    pub(super) fn from_entry(entry: &HistoryEntry) -> Option<Self> {
        let field = |name| entry.details.get(name).unwrap_or(&Value::Null);
        Some(match field("status").as_str()? {
            "queued" => Self::Queued,
            "running" => Self::Running {
                started: field("ts")
                    .as_str()
                    .and_then(|date| OffsetDateTime::parse(date, &Rfc3339).ok()),
                provider_limit: field("provider_limit").as_bool().unwrap_or(false),
            },
            "completed" => Self::Completed {
                elapsed_ms: field("elapsed_ms").as_u64(),
                input_tokens: field("estimated_input_tokens_before")
                    .as_u64()
                    .zip(field("estimated_input_tokens_after").as_u64()),
                summary_output_tokens: field("summary_output_tokens").as_u64(),
                messages: field("checkpointed_messages").as_u64(),
                summary: capped(entry.text.as_deref(), MAX_BODY_BYTES),
            },
            status @ ("failed" | "cancelled" | "interrupted") => Self::Ended {
                elapsed_ms: field("elapsed_ms").as_u64(),
                outcome: match status {
                    "failed" => End::Failed,
                    "cancelled" => End::Cancelled,
                    _ => End::Interrupted,
                },
                reason: capped(field("reason").as_str(), 512),
                continues: field("continues_with_previous_context")
                    .as_bool()
                    .unwrap_or(false),
            },
            "restored" => Self::Restored,
            _ => return None,
        })
    }

    pub(super) fn update(&mut self, candidate: Self) {
        if candidate.phase() > self.phase() {
            *self = candidate;
        }
    }

    fn phase(&self) -> u8 {
        match self {
            Self::Queued => 0,
            Self::Running { .. } => 1,
            Self::Ended {
                elapsed_ms: None, ..
            } => 2,
            Self::Completed { .. } | Self::Ended { .. } | Self::Restored => 3,
        }
    }

    pub(super) fn elapsed_seconds(&self) -> Option<u64> {
        let Self::Running {
            started: Some(started),
            ..
        } = self
        else {
            return None;
        };
        Some(
            (OffsetDateTime::now_utc() - *started)
                .whole_seconds()
                .max(0) as u64,
        )
    }

    pub(super) fn running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    pub(super) fn summary(&self) -> Option<&str> {
        match self {
            Self::Completed { summary, .. } if !summary.trim().is_empty() => Some(summary),
            _ => None,
        }
    }

    pub(super) fn render(
        &self,
        width: usize,
        open: bool,
        elapsed_seconds: Option<u64>,
    ) -> Vec<Row> {
        let (title, color, body) = match self {
            Self::Queued => ("Compaction queued", MUTED, Line::default()),
            Self::Running { provider_limit, .. } => (
                if *provider_limit {
                    "Compacting context after provider limit"
                } else {
                    "Compacting context"
                },
                ACCENT,
                elapsed_seconds.map_or_else(Line::default, |elapsed| {
                    Row::metric("", elapsed.to_string(), "s elapsed", TranscriptTone::Value)
                }),
            ),
            Self::Completed {
                elapsed_ms,
                input_tokens,
                summary_output_tokens,
                messages,
                summary,
            } => {
                let frame = CardFrame::timeline(width);
                let width = frame.content_width();
                let mut heading = Row::caption("Context compacted", "", ACCENT, width);
                let room = width - heading.line.width();
                if let Some(elapsed) = elapsed_ms.filter(|_| room > 4) {
                    let timing = Row::metric(" · ", duration(elapsed), "s", TranscriptTone::Value);
                    heading.line.extend(Row::fit(timing, room).spans);
                }
                let mut rows = vec![heading];
                if let Some((before, after)) = input_tokens {
                    rows.push(context_tokens(*before, *after, width));
                }
                if let Some(tokens) = summary_output_tokens {
                    rows.push(summary_tokens(*tokens, width));
                }
                if let Some(count) = messages {
                    let unit = if *count == 1 {
                        " message summarized"
                    } else {
                        " messages summarized"
                    };
                    rows.push(Row::plain(Row::fit(
                        Row::metric("", count.to_string(), unit, TranscriptTone::Value),
                        width,
                    )));
                }
                if open && !summary.trim().is_empty() {
                    if rows.len() > 1 {
                        rows.push(Row::plain(Line::default()));
                    }
                    rows.extend(message::markdown_body(summary, width));
                }
                return Row::card(rows, &frame);
            }
            Self::Ended {
                elapsed_ms,
                outcome,
                reason,
                continues,
            } => {
                let mut body = elapsed_ms.map_or_else(Line::default, |elapsed| {
                    Row::metric("", duration(elapsed), "s", TranscriptTone::Value)
                });
                let diagnostic = (!reason.is_empty()).then(|| single_line(reason));
                let next = matches!(outcome, End::Failed).then(|| {
                    if *continues {
                        "Continuing with previous context"
                    } else {
                        "Retry with /compact retry"
                    }
                    .to_owned()
                });
                for text in [diagnostic, next].into_iter().flatten() {
                    if body.width() > 0 {
                        body.push_span(Span::styled(" · ", TranscriptTone::Label.style()));
                    }
                    body.push_span(Span::styled(text, TranscriptTone::Text.style()));
                }
                match outcome {
                    End::Failed => ("Compaction failed", WARNING, body),
                    End::Interrupted => ("Compaction interrupted", MUTED, body),
                    End::Cancelled => ("Compaction cancelled", MUTED, body),
                }
            }
            Self::Restored => (
                "Context restored",
                SUCCESS,
                Line::from("Using the context from before the latest compaction."),
            ),
        };
        let frame = CardFrame::timeline(width);
        let width = frame.content_width();
        let mut rows = vec![Row::caption(title, "", color, width)];
        if body.width() > 0 {
            rows.extend(render_rows(vec![body], width, MAX_VISUAL_ROWS, 0));
        }
        Row::card(rows, &frame)
    }
}

fn context_tokens(before: u64, after: u64, width: usize) -> Row {
    let muted = TranscriptTone::Label.style();
    let usage = TranscriptTone::Value.emphasized();
    let before = format!("~{}", compact_token_count(before));
    let after = format!("~{}", compact_token_count(after));
    let arrow = " → ";
    let mut line = Line::from(Span::styled(shorten(&before, width), usage));
    let room = width.saturating_sub(line.width() + arrow.width());
    if room > 0 {
        line.push_span(Span::styled(arrow, muted));
        line.push_span(Span::styled(shorten(&after, room), usage));
        let room = width - line.width();
        if let Some(unit) = [" input tokens", " tokens"]
            .into_iter()
            .find(|unit| unit.len() <= room)
        {
            line.push_span(Span::styled(unit, muted));
        }
    }
    Row::plain(line)
}

fn summary_tokens(tokens: u64, width: usize) -> Row {
    let count = shorten(&tokens.to_string(), width);
    let room = width - count.width();
    let unit = [" output tokens", " tokens", ""]
        .into_iter()
        .find(|unit| unit.len() <= room)
        .unwrap_or_default();
    let label = "Summary · ";
    let label = if unit.len() + label.width() <= room {
        label
    } else {
        ""
    };
    Row::plain(Row::metric(label, count, unit, TranscriptTone::Value))
}

fn duration(ms: u64) -> String {
    format!("{:.1}", ms as f64 / 1000.0)
}
