use kuku_client::Timing;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::{App, DeliveryRow, SubmissionStatus};
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap_bounded};
use crate::text::{shorten, single_line};
use crate::theme::{emphasis, ERROR, MUTED};

const MAX_DELIVERY_ROWS: usize = 6;
const MAX_MESSAGE_ROWS: usize = 3;

pub(super) fn lines(app: &App, width: usize, limit: u16) -> Vec<Line<'static>> {
    let rows = app.pane().deliveries.rows();
    let limit = usize::from(limit).min(MAX_DELIVERY_ROWS);
    if rows.is_empty() || limit == 0 || width == 0 {
        return Vec::new();
    }
    if limit == 1 {
        return message(rows.last().expect("pending input"), width, 1);
    }
    let visible = rows.len().min(limit - 1);
    let shown = &rows[rows.len() - visible..];
    let mut title = "Pending inputs".to_owned();
    if rows.len() > visible {
        title.push_str(&format!(" · {} more", rows.len() - visible));
    }
    if app.has_queued_input() {
        title.push_str(&format!(
            " · {} /queued Edit · /withdraw",
            Shortcut::UP.label
        ));
    } else {
        title.push_str(" · /pending");
    }
    let mut lines = vec![Line::styled(shorten(&title, width), emphasis(MUTED))];
    let per_message = ((limit - 1) / visible).min(MAX_MESSAGE_ROWS);
    for row in shown {
        lines.extend(message(row, width, per_message));
    }
    lines
}

fn message(row: &DeliveryRow, width: usize, height: usize) -> Vec<Line<'static>> {
    let label = if let Some(status) = row.withdrawal {
        match status {
            SubmissionStatus::Saving | SubmissionStatus::AwaitingReceipt => "Withdrawing",
            SubmissionStatus::CheckingReceipt => "Checking withdrawal",
            SubmissionStatus::ReceiptUnconfirmed => "Withdrawal unconfirmed",
            SubmissionStatus::ReceiptMissing => "No withdrawal receipt",
            SubmissionStatus::Rejected => "Withdrawal rejected",
            status => status.label(),
        }
    } else {
        match (row.status, row.timing, row.replacement) {
            (SubmissionStatus::Saving | SubmissionStatus::AwaitingReceipt, _, true) => {
                "Sending edit"
            }
            (SubmissionStatus::Saving | SubmissionStatus::AwaitingReceipt, _, false) => "Sending",
            (SubmissionStatus::Queued, Timing::Steer, _) => "Queued · steer",
            (SubmissionStatus::Queued, Timing::NextTurn, _) => "Queued · next Turn",
            (SubmissionStatus::Rejected, _, true) => "Edit rejected",
            (status, _, _) => status.label(),
        }
    };
    let style = if matches!(
        row.withdrawal.unwrap_or(row.status),
        SubmissionStatus::Rejected
            | SubmissionStatus::ReceiptMissing
            | SubmissionStatus::ReceiptUnconfirmed
    ) {
        emphasis(ERROR)
    } else {
        emphasis(MUTED)
    };
    let heading = format!("You · {label}");
    if height == 1 {
        return vec![Line::styled(
            shorten(&format!("{heading} · {}", single_line(&row.text)), width),
            style,
        )];
    }
    let mut lines = vec![Line::from(Span::styled(shorten(&heading, width), style))];
    let content = wrap_bounded(
        render_literal(&row.text, Style::default()),
        width,
        height - 1,
    );
    let clipped = content.len() + 1 > height;
    lines.extend(content.into_iter().take(height - 1));
    if clipped {
        let last = lines.last_mut().expect("visible message");
        *last = Line::raw(format!(
            "{}…",
            shorten(&last.to_string(), width.saturating_sub(1)).trim_end_matches('…')
        ));
    }
    lines
}
