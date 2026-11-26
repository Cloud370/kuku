use std::ops::Range;

use kuku_client::Timing;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, DeliveryRow, KeyboardFocus, SubmissionStatus};
use crate::contextual::ContextControl;
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap_bounded};
use crate::text::{shorten, single_line};
use crate::theme::{ACCENT, ERROR, MUTED, emphasis, focused_row};

const MAX_DELIVERY_ROWS: usize = 6;
const MAX_MESSAGE_ROWS: usize = 3;

pub(super) struct DeliveryView {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) hits: Vec<(Range<u16>, String)>,
    pub(super) action: Option<(Range<u16>, ContextControl)>,
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
            action: None,
        };
    }
    let controls = app.contextual_controls();
    let run = controls.iter().find(|control| {
        matches!(
            control,
            ContextControl::ReviewBlocked { .. } | ContextControl::Continue { .. }
        )
    });
    let target = app.selection().target;
    let recovery = controls
        .iter()
        .find(|control| control.recovers_target(&target));
    let online = app.service_status() == crate::app::ServiceStatus::Connected;
    // Review remains locally available. Never offer Advance ahead of an
    // unresolved receipt, or when the service cannot accept the command.
    let action_control = run
        .filter(|control| matches!(control, ContextControl::ReviewBlocked { .. }))
        .or(recovery.filter(|control| control.is_interrupt_recovery()))
        .or(run.filter(|_| online && !rows.is_empty()))
        .or(recovery);
    if rows.is_empty() && action_control.is_none() {
        let stale = selected_id.is_some_and(|id| !app.queue_input_available(id));
        return DeliveryView {
            lines: if stale {
                vec![Line::styled(
                    "Selected input is no longer queued",
                    emphasis(ERROR),
                )]
            } else {
                vec![]
            },
            hits: Vec::new(),
            action: None,
        };
    }
    if rows.is_empty() {
        let control = action_control.expect("an empty delivery view needs a recovery action");
        let (line, action) = action_title("", "", width, app, control);
        return DeliveryView {
            lines: vec![line],
            hits: Vec::new(),
            action,
        };
    }

    let saved = rows
        .iter()
        .filter(|row| row.status == SubmissionStatus::Queued)
        .count();
    let state = if matches!(run, Some(ContextControl::ReviewBlocked { .. })) {
        "Tool result unknown"
    } else if !online {
        "Offline"
    } else if app
        .pane()
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.cleaning_up)
    {
        "Stopping"
    } else if matches!(run, Some(ContextControl::Continue { .. })) {
        "Paused"
    } else {
        ""
    };
    let compact_title = match (saved, state) {
        (0, state) => state.to_owned(),
        (count, "") => format!("{count} saved"),
        (count, state) => format!("{count} saved · {state}"),
    };
    let full_title = if compact_title.is_empty() {
        "Pending inputs".to_owned()
    } else {
        format!("Pending inputs · {compact_title}")
    };
    if limit == 1
        && let Some(control) = action_control
    {
        let (line, action) = action_title(&full_title, &compact_title, width, app, control);
        return DeliveryView {
            lines: vec![line],
            hits: Vec::new(),
            action,
        };
    }
    let stale_without_row = selected_id
        .is_some_and(|id| !app.queue_input_available(id) && !rows.iter().any(|row| row.id == id));
    if limit == 1 {
        if stale_without_row {
            return DeliveryView {
                lines: vec![Line::styled(
                    "Selected input is no longer queued",
                    emphasis(ERROR),
                )],
                hits: Vec::new(),
                action: None,
            };
        }
        let index = selected_id
            .and_then(|id| rows.iter().position(|row| row.id == id))
            .unwrap_or(rows.len() - 1);
        let selected = Some(rows[index].id.as_str()) == selected_id;
        let stale = selected && !app.queue_input_available(&rows[index].id);
        let hits = if app.queue_input_available(&rows[index].id) {
            vec![(0..1, rows[index].id.clone())]
        } else {
            Vec::new()
        };
        return DeliveryView {
            lines: message(&rows[index], width, 1, selected, stale),
            hits,
            action: None,
        };
    }
    let separate_hints = action_control.is_some() && limit >= 3 && app.has_queued_input();
    let message_rows = limit - 1 - usize::from(separate_hints);
    let visible = rows.len().min(message_rows);
    let selected = selected_id.and_then(|id| rows.iter().position(|row| row.id == id));
    let start = selected.map_or(rows.len() - visible, |index| {
        index.saturating_sub(visible / 2).min(rows.len() - visible)
    });
    let shown = &rows[start..start + visible];
    let mut title = if stale_without_row {
        format!("Selected input is no longer queued · {full_title}")
    } else {
        full_title
    };
    if rows.len() > visible {
        title.push_str(&format!(
            " · {}–{} of {}",
            start + 1,
            start + visible,
            rows.len()
        ));
    }
    let mut hints = String::new();
    if app.has_queued_input() {
        hints.push_str(&format!("{} select", Shortcut::UP.label));
        if selected_id.map_or_else(
            || {
                rows.iter().any(|row| {
                    row.agent_sender.is_none()
                        && row.status == SubmissionStatus::Queued
                        && app.queue_input_editable(&row.id)
                })
            },
            |id| app.queue_input_editable(id),
        ) {
            hints.push_str(" · Enter edit");
        }
        hints.push_str(" · Del remove");
        if action_control.is_none() {
            title.push_str(" · ");
            title.push_str(&hints);
        }
    }
    let (title, action) = action_control.map_or_else(
        || (Line::styled(shorten(&title, width), emphasis(MUTED)), None),
        |control| action_title(&title, &compact_title, width, app, control),
    );
    let mut lines = vec![title];
    let mut hits = Vec::new();
    let per_message = (message_rows / visible).min(MAX_MESSAGE_ROWS);
    for row in shown {
        let selected = Some(row.id.as_str()) == selected_id;
        let stale = selected && !app.queue_input_available(&row.id);
        let first = lines.len();
        lines.extend(message(row, width, per_message, selected, stale));
        if app.queue_input_available(&row.id) {
            hits.push((first as u16..lines.len() as u16, row.id.clone()));
        }
    }
    if separate_hints {
        lines.push(Line::styled(shorten(&hints, width), emphasis(MUTED)));
    }
    DeliveryView {
        lines,
        hits,
        action,
    }
}

fn action_title(
    title: &str,
    compact: &str,
    width: usize,
    app: &App,
    control: &ContextControl,
) -> (Line<'static>, Option<(Range<u16>, ContextControl)>) {
    let name = match control {
        ContextControl::Continue { .. } if app.is_subagent() => "Continue agent",
        ContextControl::Continue { .. } => "Continue session",
        ContextControl::ReviewBlocked { .. } if width < 42 => "Review tool result",
        other => super::contextual::action_label(other),
    };
    let separator = " · ";
    let focused = app.pane().contextual_focus.active
        && app.pane().contextual_focus.cursor.as_ref() == Some(control);
    let key = if focused { " · Enter" } else { " · F2" };
    let fits = |prefix: &str, action: &str| {
        prefix.width() + usize::from(!prefix.is_empty()) * separator.width() + action.width()
            <= width
    };
    let (prefix, action) = if fits(title, name) {
        (title.to_owned(), name.to_owned())
    } else if !compact.is_empty() && fits(compact, name) {
        (compact.to_owned(), name.to_owned())
    } else if let Some((count, _)) = compact.split_once(separator)
        && fits(count, name)
    {
        (count.to_owned(), name.to_owned())
    } else {
        // Keep the action readable even when the state cannot also fit.
        (String::new(), shorten(name, width))
    };
    let style = if focused {
        focused_row()
    } else {
        emphasis(ACCENT)
    };
    let has_prefix = !prefix.is_empty();
    let mut line = Line::styled(prefix, emphasis(MUTED));
    if has_prefix {
        line.push_span(separator);
    }
    let start = line.width();
    let end = start + action.width();
    line.push_span(Span::styled(action, style));
    if end + key.width() <= width {
        line.push_span(Span::styled(key, emphasis(MUTED)));
    }
    (
        line,
        (end > start).then(|| (start as u16..end as u16, control.clone())),
    )
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
            SubmissionStatus::Saving | SubmissionStatus::AwaitingReceipt => "Removing from queue",
            SubmissionStatus::CheckingReceipt => "Checking if removed",
            SubmissionStatus::ReceiptUnconfirmed => "Removal unconfirmed",
            SubmissionStatus::ReceiptMissing => "No removal receipt",
            SubmissionStatus::Rejected => "Could not remove",
            status => status.label(),
        }
    } else {
        match (row.status, row.timing, row.replacement) {
            (SubmissionStatus::Saving | SubmissionStatus::AwaitingReceipt, _, true) => {
                "Sending edit"
            }
            (SubmissionStatus::Saving | SubmissionStatus::AwaitingReceipt, _, false) => "Sending",
            (SubmissionStatus::Queued, Timing::Steer, _) => "Queued · next step",
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
    let sender = row.agent_sender.as_deref().map_or_else(
        || "You".to_owned(),
        |agent| format!("Agent {}", single_line(agent)),
    );
    let heading = if stale {
        format!("{sender} · No longer queued")
    } else {
        format!("{}{sender} · {label}", if selected { "› " } else { "  " })
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
    let focused = app.pane().keyboard_focus == KeyboardFocus::Request;
    if let Some(request) = app.pane().question() {
        let agent = single_line(&request.agent);
        let hint = if !app.available() {
            "Last seen waiting"
        } else if focused {
            "Enter reply · Esc back"
        } else {
            "F2 Reply"
        };
        let title = if app.available() {
            format!("Question · Waiting · {agent} · {hint}")
        } else {
            format!("Question · {agent} · {hint}")
        };
        let compact = format!("Question · {hint}");
        let title_style = if focused {
            focused_row().fg(crate::theme::WARNING)
        } else {
            emphasis(crate::theme::WARNING)
        };
        let mut lines = if title.width() <= width {
            vec![Line::styled(title, title_style)]
        } else if limit > 1
            && format!("Question · {agent}").width() <= width
            && hint.width() <= width
        {
            let status = if app.available() && format!("Waiting · {hint}").width() <= width {
                format!("Waiting · {hint}")
            } else {
                hint.to_owned()
            };
            vec![
                Line::styled(format!("Question · {agent}"), title_style),
                Line::styled(status, Style::default().fg(crate::theme::WARNING)),
            ]
        } else {
            vec![Line::styled(shorten(&compact, width), title_style)]
        };
        if app.pane().transcript.contains_question(&request.id) {
            // Keep a cue at the dock when the full question has scrolled out of
            // view, but never duplicate its complete text in the transcript.
            let excerpt = single_line(&request.text);
            if lines.len() < usize::from(limit) && excerpt.width() >= 8 {
                let budget = width.min(48).min(excerpt.width().saturating_sub(1));
                lines.push(Line::styled(
                    shorten(&excerpt, budget),
                    Style::default().fg(MUTED),
                ));
            }
        } else {
            let remaining = usize::from(limit).saturating_sub(lines.len());
            lines.extend(wrap_bounded(
                render_literal(&request.text, Style::default()),
                width,
                remaining,
            ));
        }
        return RequestView {
            lines: lines.into_iter().take(limit as usize).collect(),
            permission_choices: None,
            detail_page_size: 0,
        };
    }
    let Some(request) = app.pane().permission() else {
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
