use kuku_client::{HistoryEntry, JobStatus};
use serde::Deserialize;
use serde_json::Value;

use crate::jobs::job_status_label;
use crate::theme::{ERROR, MUTED, SUCCESS, WARNING};

use super::{capped, Body, MAX_BODY_BYTES};

pub(super) fn job_notice(entry: &HistoryEntry) -> Body {
    let id = entry.details.get("job_id").and_then(Value::as_str);
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
        id.unwrap_or_default(),
        status.map_or("Finished", job_status_label),
        summary.or(entry.text.as_deref()).unwrap_or_default(),
    ];
    let text = details
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    Body::Notice("Background job", capped(Some(&text), MAX_BODY_BYTES), color)
}
