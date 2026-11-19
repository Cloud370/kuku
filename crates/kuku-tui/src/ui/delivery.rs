use std::ops::Range;

use kuku_client::Timing;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, DeliveryRow, KeyboardFocus, SubmissionStatus};
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap_bounded};
use crate::text::{shorten, single_line};
use crate::theme::{emphasis, focused_row, ERROR, MUTED};

const MAX_DELIVERY_ROWS: usize = 6;
const MAX_MESSAGE_ROWS: usize = 3;

pub(super) struct DeliveryView {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) hits: Vec<(Range<u16>, String)>,
}

pub(super) struct RequestView {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) permission_choices: Option<(usize, Range<usize>, Range<usize>)>,
    pub(super) detail_page_size: usize,
}

pub(super) fn lines(app: &App, width: usize, limit: u16) -> DeliveryView {
    let rows = app.pane().deliveries.rows();
    let selection_active = app.pane().keyboard_focus == KeyboardFocus::Queue
        || matches!(&app.modal, Some(crate::app::Modal::Prompt(prompt))
            if matches!(&prompt.kind, crate::menu::PromptKind::QueuedInput { .. }));
    let selected_id = selection_active
        .then(|| app.pane().selected_queue_input.as_deref())
        .flatten();
    let limit = usize::from(limit).min(MAX_DELIVERY_ROWS);
    if limit == 0 || width == 0 {
        return DeliveryView {
            lines: Vec::new(),
            hits: Vec::new(),
        };
    }
    if rows.is_empty() {
        let stale = selected_id.is_some_and(|id| {
            !app.queued_for_edit()
                .any(|entry| &entry.receipt.command_id == id)
        });
        return DeliveryView {
            lines: stale
                .then(|| Line::styled("Selected input is no longer queued", emphasis(ERROR)))
                .into_iter()
                .collect(),
            hits: Vec::new(),
        };
    }
    let stale_without_row = selected_id.is_some_and(|id| {
        !app.queued_for_edit()
            .any(|entry| &entry.receipt.command_id == id)
            && !rows.iter().any(|row| &row.id == id)
    });
    if limit == 1 {
        if stale_without_row {
            return DeliveryView {
                lines: vec![Line::styled(
                    "Selected input is no longer queued",
                    emphasis(ERROR),
                )],
                hits: Vec::new(),
            };
        }
        let index = selected_id
            .and_then(|id| rows.iter().position(|row| row.id == id))
            .unwrap_or(rows.len() - 1);
        let selected = Some(rows[index].id.as_str()) == selected_id;
        let stale = selected
            && !app
                .queued_for_edit()
                .any(|entry| entry.receipt.command_id == rows[index].id);
        let hits = if app
            .queued_for_edit()
            .any(|entry| entry.receipt.command_id == rows[index].id)
        {
            vec![(0..1, rows[index].id.clone())]
        } else {
            Vec::new()
        };
        return DeliveryView {
            lines: message(&rows[index], width, 1, selected, stale),
            hits,
        };
    }
    let visible = rows.len().min(limit - 1);
    let selected = selected_id.and_then(|id| rows.iter().position(|row| row.id == id));
    let start = selected.map_or(rows.len() - visible, |index| {
        index.saturating_sub(visible / 2).min(rows.len() - visible)
    });
    let shown = &rows[start..start + visible];
    let mut title = if stale_without_row {
        "Selected input is no longer queued · Pending inputs".to_owned()
    } else {
        "Pending inputs".to_owned()
    };
    if rows.len() > visible {
        title.push_str(&format!(
            " · {}–{} of {}",
            start + 1,
            start + visible,
            rows.len()
        ));
    }
    if app.queued_for_edit().next().is_some() {
        title.push_str(&format!(
            " · {} select · Enter edit · Del withdraw",
            Shortcut::UP.label
        ));
    } else {
        title.push_str(" · /pending");
    }
    let mut lines = vec![Line::styled(shorten(&title, width), emphasis(MUTED))];
    let mut hits = Vec::new();
    let per_message = ((limit - 1) / visible).min(MAX_MESSAGE_ROWS);
    for row in shown {
        let selected = Some(row.id.as_str()) == selected_id;
        let stale = selected
            && !app
                .queued_for_edit()
                .any(|entry| entry.receipt.command_id == row.id);
        let first = lines.len();
        lines.extend(message(row, width, per_message, selected, stale));
        if app
            .queued_for_edit()
            .any(|entry| entry.receipt.command_id == row.id)
        {
            hits.push((first as u16..lines.len() as u16, row.id.clone()));
        }
    }
    DeliveryView { lines, hits }
}

fn message(
    row: &DeliveryRow,
    width: usize,
    height: usize,
    selected: bool,
    stale: bool,
) -> Vec<Line<'static>> {
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
    let heading = if stale {
        "You · No longer queued".to_owned()
    } else {
        format!("{}You · {label}", if selected { "› " } else { "  " })
    };
    let heading_style = if selected { focused_row() } else { style };
    if height == 1 {
        return vec![Line::styled(
            shorten(&format!("{heading} · {}", single_line(&row.text)), width),
            heading_style,
        )];
    }
    let mut lines = vec![Line::from(Span::styled(
        shorten(&heading, width),
        heading_style,
    ))];
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

pub(super) fn request_lines(app: &App, width: usize, limit: u16) -> RequestView {
    if width == 0 || limit == 0 {
        return RequestView {
            lines: Vec::new(),
            permission_choices: None,
            detail_page_size: 0,
        };
    }
    let Some(snapshot) = app.pane().snapshot.as_ref() else {
        return RequestView {
            lines: Vec::new(),
            permission_choices: None,
            detail_page_size: 0,
        };
    };
    let focused = app.pane().keyboard_focus == KeyboardFocus::Request;
    if let Some(request) = &snapshot.agent.question {
        let title = if focused {
            "Question · Enter answer · Esc back"
        } else {
            "Question · F2 focus"
        };
        let mut lines = vec![Line::styled(
            shorten(title, width),
            emphasis(crate::theme::WARNING),
        )];
        if limit > 1 {
            lines.push(Line::raw(shorten(&single_line(&request.text), width)));
        }
        return RequestView {
            lines: lines.into_iter().take(limit as usize).collect(),
            permission_choices: None,
            detail_page_size: 0,
        };
    }
    let Some(request) = &snapshot.agent.permission else {
        return RequestView {
            lines: Vec::new(),
            permission_choices: None,
            detail_page_size: 0,
        };
    };
    let title = if focused {
        format!(
            "Permission · {} · {}",
            single_line(&request.tool),
            single_line(&request.risk)
        )
    } else {
        format!(
            "Permission · {} · {} · F2 focus · ←/→ choose · Enter decide · Esc back",
            single_line(&request.tool),
            single_line(&request.risk)
        )
    };
    let choice = app.pane().permission_choice;
    let mut lines = vec![Line::styled(
        shorten(&title, width),
        emphasis(crate::theme::WARNING),
    )];
    let mut detail_page_size = 0;
    if focused {
        if limit > 2 {
            lines.push(Line::raw(shorten(
                "←/→ choose · Enter decide · Esc back · PgUp/PgDn details",
                width,
            )));
        }
        let detail_height = usize::from(limit.saturating_sub(3));
        detail_page_size = detail_height;
        if detail_height > 0 {
            let details = format!(
                "Summary: {}\nCandidate:\n{}",
                single_line(&request.summary),
                request.candidate
            );
            let offset = app.pane().request_scroll;
            let visible = wrap_bounded(
                render_literal(&details, Style::default()),
                width,
                offset.saturating_add(detail_height).saturating_add(1),
            );
            lines.extend(visible.into_iter().skip(offset).take(detail_height));
        }
    } else if limit > 2 {
        lines.push(Line::raw(shorten(
            &format!("Summary: {}", single_line(&request.summary)),
            width,
        )));
    }
    let mut permission_choices = None;
    if limit > 1 {
        let deny_prefix = if focused && choice == Some(false) {
            "› "
        } else {
            "  "
        };
        let allow_prefix = if focused && choice == Some(true) {
            "› "
        } else {
            "  "
        };
        let deny_start = deny_prefix.width();
        let deny_range = deny_start..deny_start + "Deny".width();
        let allow_start = deny_range.end + "     ".width() + allow_prefix.width();
        let allow_range = allow_start..allow_start + "Allow once".width();
        let row = lines.len();
        lines.push(Line::from(vec![
            Span::raw(deny_prefix),
            Span::styled(
                "Deny",
                if focused && choice == Some(false) {
                    focused_row()
                } else {
                    Style::default()
                },
            ),
            Span::raw("     "),
            Span::raw(allow_prefix),
            Span::styled(
                "Allow once",
                if focused && choice == Some(true) {
                    focused_row()
                } else {
                    Style::default()
                },
            ),
        ]));
        permission_choices = Some((row, deny_range, allow_range));
    }
    RequestView {
        lines: lines.into_iter().take(limit as usize).collect(),
        permission_choices,
        detail_page_size,
    }
}

pub(super) fn choice_area(
    area: ratatui::layout::Rect,
    row: usize,
    range: Range<usize>,
) -> ratatui::layout::Rect {
    let offset = u16::try_from(range.start).unwrap_or(u16::MAX);
    if offset >= area.width {
        return ratatui::layout::Rect::ZERO;
    }
    let width = u16::try_from(range.end.saturating_sub(range.start))
        .unwrap_or(u16::MAX)
        .min(area.width - offset);
    ratatui::layout::Rect::new(
        area.x.saturating_add(offset),
        area.y.saturating_add(row as u16),
        width,
        1,
    )
}
