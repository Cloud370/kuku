use kuku_client::{HistoryEntry, HistoryKind};
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::markup::render_literal;
use crate::theme::{CardFrame, ERROR, WARNING};

use super::{Body, MAX_VISUAL_ROWS, Row, message, render_rows};

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
        retry_message: None,
        request_id: entry
            .details
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        reply_interrupted: false,
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
            fallback_hint,
            retry_message,
            reply_interrupted,
            ..
        } => {
            let mut notice = String::new();
            let title = if *reply_interrupted {
                let combined = format!("{title} · Reply interrupted");
                let frame = CardFrame::timeline(width);
                let available = frame.title_width().min(frame.content_width());
                if combined.width() <= available {
                    combined
                } else {
                    // The correlated standalone marker is hidden, so the
                    // interruption must remain visible even at narrow widths.
                    notice.push_str(title);
                    notice.push('\n');
                    message::interruption_title(available, true).to_owned()
                }
            } else {
                (*title).to_owned()
            };
            if *reply_interrupted {
                let message = retry_message
                    .as_deref()
                    .filter(|message| !message.is_empty());
                if let Some(message) = message {
                    notice.push_str(message);
                } else if !fallback_hint.is_empty() {
                    notice.push_str("Next step: ");
                    notice.push_str(fallback_hint);
                }
                let explanation = message.unwrap_or(fallback_hint);
                if !diagnostic.is_empty() && diagnostic != explanation {
                    if !notice.is_empty() {
                        notice.push_str("\n\n");
                        notice.push_str("Diagnostic: ");
                    }
                    notice.push_str(diagnostic);
                }
            } else {
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
            }
            Some(message::render_activity_record(
                &title, ERROR, &notice, width,
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
    let frame = CardFrame::timeline(width);
    let mut rows = Vec::new();
    if heading {
        rows.push(Row::plain(frame.title("Retry history", WARNING)));
    }
    rows.extend(
        render_rows(
            render_literal(text, Style::default()),
            frame.content_width(),
            MAX_VISUAL_ROWS,
            0,
        )
        .map(|row| row.framed(&frame)),
    );
    if ends {
        rows.extend(Row::frame_end(&frame));
    }
    rows
}
