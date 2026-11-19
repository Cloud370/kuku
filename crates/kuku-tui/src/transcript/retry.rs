use kuku_client::{HistoryEntry, HistoryKind};
use ratatui::style::Style;

use crate::markup::render_literal;
use crate::theme::{ERROR, WARNING};

use super::{render_record, render_rows, Body, Row, MAX_VISUAL_ROWS};

pub(super) fn interruption_execution(entry: &HistoryEntry) -> Option<u64> {
    if entry.kind == HistoryKind::Status
        && entry
            .details
            .get("status")
            .and_then(serde_json::Value::as_str)
            == Some("interrupted")
    {
        return entry.details.get("execution_id")?.as_u64();
    }
    None
}

pub(super) fn error_body(entry: &HistoryEntry, diagnostic: String) -> Body {
    let code = entry
        .details
        .get("code")
        .and_then(serde_json::Value::as_str);
    let hint = crate::retry_notice::model_error_hint(code).to_owned();
    Body::ModelError {
        title: crate::retry_notice::model_error_title(code),
        diagnostic,
        fallback_hint: hint.clone(),
        hint,
        request_id: entry
            .details
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

pub(super) fn retry_body(entry: &HistoryEntry) -> Body {
    let metadata = entry
        .details
        .get("retry")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    Body::Retry {
        metadata,
        include_detail: true,
    }
}

pub(super) fn render(
    body: &Body,
    width: usize,
    retry_heading: bool,
    retry_end: bool,
) -> Option<Vec<Row>> {
    match body {
        Body::ModelError {
            title,
            diagnostic,
            hint,
            ..
        } => {
            let mut notice = String::new();
            if !diagnostic.is_empty() {
                notice.push_str(diagnostic);
            }
            if !hint.is_empty() {
                if !notice.is_empty() {
                    notice.push_str("\n\n");
                }
                notice.push_str("Next step: ");
                notice.push_str(hint);
            }
            Some(render_record(
                title,
                ERROR,
                render_literal(&notice, Style::default()),
                width,
                0,
            ))
        }
        Body::Retry {
            metadata,
            include_detail,
        } => Some(render_retry_history(
            &crate::retry_notice::history_text(metadata, *include_detail),
            width,
            retry_heading,
            retry_end,
        )),
        _ => None,
    }
}

fn render_retry_history(text: &str, width: usize, heading: bool, ends: bool) -> Vec<Row> {
    let lines = render_literal(text, Style::default());
    let mut rows = if heading {
        render_record("Retry history", WARNING, lines, width, 0)
    } else {
        render_rows(lines, width, MAX_VISUAL_ROWS, 0).collect()
    };
    if !ends && rows.last().is_some_and(|row| row.line.spans.is_empty()) {
        rows.pop();
    } else if ends && rows.last().is_none_or(|row| !row.line.spans.is_empty()) {
        rows.push(Row::plain(ratatui::text::Line::default()));
    }
    rows
}
