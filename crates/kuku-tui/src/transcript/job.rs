use kuku_client::{HistoryEntry, JobStatus};
use ratatui::style::Color;
use serde::Deserialize;
use serde_json::Value;

use crate::jobs::job_status_label;
use crate::text::format_duration_ms;
use crate::theme::{ERROR, MUTED, SUCCESS, WARNING};

use super::{MAX_BODY_BYTES, ToolView, capped};

#[derive(Debug, Clone)]
pub(super) struct JobNotice {
    id: String,
    pub(super) text: String,
    pub(super) color: Color,
    output_id: Option<String>,
    raw_summary: String,
    elapsed_ms: Option<u64>,
    command_duration_ms: Option<u64>,
}

impl JobNotice {
    pub(super) fn from_entry(entry: &HistoryEntry) -> Self {
        let id = entry
            .details
            .get("job_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let summary = entry.details.get("summary").and_then(Value::as_str);
        let status = entry
            .details
            .get("status")
            .and_then(|value| JobStatus::deserialize(value).ok());
        let color = match status {
            Some(JobStatus::Completed) => SUCCESS,
            Some(JobStatus::Failed) => ERROR,
            Some(JobStatus::Cancelled | JobStatus::Interrupted) => WARNING,
            _ => MUTED,
        };
        let command = entry.details.get("command");
        let elapsed_ms = entry.details.get("elapsed_ms").and_then(Value::as_u64);
        let command_duration_ms = command.and_then(|value| value["duration_ms"].as_u64());
        let raw_summary = summary.or(entry.text.as_deref()).unwrap_or_default();
        let mut parts = vec![
            id.to_owned(),
            status.map_or("Finished", job_status_label).to_owned(),
        ];
        if let Some(command) = command {
            match command["termination"].as_str() {
                Some("exited") => {
                    if let Some(code) = command["exit_code"].as_i64() {
                        parts.push(format!("exit {code}"));
                    } else if let Some(signal) = command["signal"].as_i64() {
                        parts.push(format!("signal {signal}"));
                    }
                }
                Some("timed_out") => parts.push("timed out".into()),
                Some("program_missing") => parts.push("program missing".into()),
                Some("start_failed") => parts.push("could not start".into()),
                _ => {}
            }
        } else if !raw_summary.is_empty() {
            parts.push(raw_summary.to_owned());
        }
        if let Some(ms) = command_duration_ms.or(elapsed_ms) {
            parts.push(format!("{} total", format_duration_ms(ms)));
        }
        let text = parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let output_id = entry
            .details
            .get("output_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        Self {
            id: id.into(),
            text: capped(Some(&text), MAX_BODY_BYTES),
            color,
            output_id,
            raw_summary: capped(Some(raw_summary), MAX_BODY_BYTES),
            elapsed_ms,
            command_duration_ms,
        }
    }

    pub(super) fn label(&self) -> String {
        if self.id.is_empty() {
            "Background job".into()
        } else {
            format!("Background job · {}", self.id)
        }
    }

    pub(super) fn evidence(&self) -> String {
        [
            Some(self.raw_summary.clone()),
            self.command_duration_ms
                .map(|ms| format!("Command duration · {ms} ms")),
            self.elapsed_ms.map(|ms| format!("Job elapsed · {ms} ms")),
        ]
        .into_iter()
        .flatten()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }

    pub(super) fn output(&self) -> Option<ToolView> {
        Some(ToolView {
            name: self.id.clone(),
            summary: self.text.clone(),
            evidence: self.evidence(),
            output_id: Some(self.output_id.clone()?),
            ..ToolView::default()
        })
    }
}
