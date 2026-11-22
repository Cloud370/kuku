use kuku_client::HistoryEntry;
use ratatui::text::Line;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use crate::text::{compact_token_count, single_line};
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::{body_inset, capped, message, Row, MAX_BODY_BYTES};

#[derive(Debug, Clone)]
pub(super) enum Compaction {
    Queued,
    Running {
        started: Option<OffsetDateTime>,
        provider_limit: bool,
    },
    Completed {
        elapsed_ms: u64,
        input_tokens: (u64, u64),
        messages: u64,
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
                elapsed_ms: field("elapsed_ms").as_u64()?,
                input_tokens: (
                    field("estimated_input_tokens_before").as_u64()?,
                    field("estimated_input_tokens_after").as_u64()?,
                ),
                messages: field("checkpointed_messages").as_u64()?,
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
        let rail = body_inset(width);
        let body_width = width.saturating_sub(usize::from(rail));
        let (title, color, body) = match self {
            Self::Queued => ("Compaction queued", MUTED, String::new()),
            Self::Running { provider_limit, .. } => (
                if *provider_limit {
                    "Compacting context after provider limit"
                } else {
                    "Compacting context"
                },
                ACCENT,
                elapsed_seconds.map_or_else(String::new, |elapsed| format!("{elapsed}s elapsed")),
            ),
            Self::Completed {
                elapsed_ms,
                input_tokens,
                messages,
                summary,
            } => {
                let counts = format!(
                    "~{} → ~{}",
                    compact_token_count(input_tokens.0),
                    compact_token_count(input_tokens.1),
                );
                let unit = if "Context compacted ·  input tokens".width() + counts.width() <= width
                {
                    "input tokens"
                } else {
                    "tokens"
                };
                let mut rows = vec![
                    Row::plain(Line::default()),
                    Row::caption(
                        "Context compacted",
                        &format!("{counts} {unit}"),
                        ACCENT,
                        width,
                    ),
                ];
                if open {
                    let count = if *messages == 1 {
                        "message"
                    } else {
                        "messages"
                    };
                    let details =
                        format!("{} · {messages} {count} summarized", duration(*elapsed_ms));
                    rows.push(Row::caption(&details, "", MUTED, body_width).with_rail(rail));
                    if !summary.trim().is_empty() {
                        rows.push(Row::plain(Line::default()).with_rail(rail));
                        let mut body = message::markdown_body(summary, body_width, 0);
                        if body.last().is_some_and(|row| row.line.spans.is_empty()) {
                            body.pop();
                        }
                        rows.extend(body.into_iter().map(|row| row.with_rail(rail)));
                    }
                }
                rows.push(Row::plain(Line::default()));
                return rows;
            }
            Self::Ended {
                elapsed_ms,
                outcome,
                reason,
                continues,
            } => {
                let elapsed = elapsed_ms.map(duration);
                let diagnostic = (!reason.is_empty()).then(|| single_line(reason));
                let next = matches!(outcome, End::Failed).then(|| {
                    if *continues {
                        "Continuing with previous context"
                    } else {
                        "Retry with /compact retry"
                    }
                    .to_owned()
                });
                let body = [elapsed, diagnostic, next]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                match outcome {
                    End::Failed => ("Compaction failed", WARNING, body),
                    End::Interrupted => ("Compaction interrupted", MUTED, body),
                    End::Cancelled => ("Compaction cancelled", MUTED, body),
                }
            }
            Self::Restored => (
                "Context restored",
                SUCCESS,
                "Using the context from before the latest compaction.".into(),
            ),
        };
        message::render_activity_record(title, color, &body, width)
    }
}

fn duration(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}
