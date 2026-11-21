use kuku_client::{HistoryEntry, JobStatus};
use serde::Deserialize;
use serde_json::Value;

use crate::jobs::job_status_label;
use crate::theme::{ERROR, MUTED, SUCCESS, WARNING};

use super::{capped, ToolView, MAX_BODY_BYTES};

#[derive(Debug, Clone)]
pub(super) struct JobNotice {
    id: String,
    pub(super) text: String,
    pub(super) color: Color,
    output_id: Option<String>,
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
        let details = [
            id,
            status.map_or("Finished", job_status_label),
            summary.or(entry.text.as_deref()).unwrap_or_default(),
        ];
        let text = details
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
        }
    }

    pub(super) fn label(&self) -> String {
        if self.id.is_empty() {
            "Background job".into()
        } else {
            format!("Background job · {}", self.id)
        }
    }

    pub(super) fn output(&self) -> Option<ToolView> {
        Some(ToolView {
            name: self.id.clone(),
            summary: self.text.clone(),
            output_id: Some(self.output_id.clone()?),
            ..ToolView::default()
        })
    }
}
use ratatui::style::Color;
