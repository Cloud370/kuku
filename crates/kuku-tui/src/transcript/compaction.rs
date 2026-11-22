use kuku_client::HistoryEntry;
use ratatui::style::Style;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::markup::render_literal;
use crate::text::{shorten, single_line};
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::{capped, message, render_record, Row, MAX_BODY_BYTES};

#[derive(Debug, Clone)]
pub(super) enum Compaction {
    Queued,
    Running {
        started: Option<OffsetDateTime>,
    },
    Completed {
        elapsed_ms: u64,
        before: u64,
        after: u64,
        messages: u64,
        summary: String,
    },
    Ended {
        elapsed_ms: Option<u64>,
        outcome: End,
        reason: String,
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
            },
            "completed" => Self::Completed {
                elapsed_ms: field("elapsed_ms").as_u64()?,
                before: field("before_bytes").as_u64()?,
                after: field("after_bytes").as_u64()?,
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
            Self::Queued => ("Compaction queued", MUTED, String::new()),
            Self::Running { .. } => (
                "Compacting context",
                ACCENT,
                elapsed_seconds.map_or_else(String::new, |elapsed| format!("{elapsed}s elapsed")),
            ),
            Self::Completed {
                elapsed_ms,
                before,
                after,
                messages,
                summary,
            } => {
                let stats = format!(
                    "{} · {} → {}\n{messages} messages summarized",
                    duration(*elapsed_ms),
                    size(*before),
                    size(*after)
                );
                if open {
                    return message::render_markdown_record(
                        "Context compacted ▾",
                        SUCCESS,
                        &format!("{stats}\n\n{summary}"),
                        width,
                    );
                }
                let preview = summary
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && !line.starts_with('#'));
                let body = preview.map_or(stats.clone(), |line| {
                    format!("{stats}\n{}", shorten(&single_line(line), 160))
                });
                (
                    if summary.is_empty() {
                        "Context compacted"
                    } else {
                        "Context compacted ▸"
                    },
                    SUCCESS,
                    body,
                )
            }
            Self::Ended {
                elapsed_ms,
                outcome,
                reason,
            } => {
                let elapsed = elapsed_ms.map(duration);
                let diagnostic = (!reason.is_empty()).then(|| single_line(reason));
                let retry = matches!(outcome, End::Failed)
                    .then_some("Retry with /compact retry".to_owned());
                let body = [elapsed, diagnostic, retry]
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
        render_record(
            title,
            color,
            render_literal(&body, Style::default()),
            width,
            0,
        )
    }
}

fn duration(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}
