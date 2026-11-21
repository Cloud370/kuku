use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::contextual::{ContextControl, ContextHit};
use crate::keys::Shortcut;
use crate::text::{compact_token_count, shorten};
use crate::theme::{emphasis, focused_row, ACCENT, MUTED, WARNING};

pub(super) fn label(control: &ContextControl) -> String {
    match control {
        ContextControl::Request { .. } => "F2 Request".into(),
        ContextControl::RepairCredentials { .. } => "F2 Repair credentials · Enter".into(),
        ContextControl::Continue { .. } => "F2 Continue · Enter".into(),
        ContextControl::Interrupt { .. } => "F2 Stop · Enter".into(),
        control @ ContextControl::PendingRecovery { .. } => {
            let title = if control.is_send_recovery() {
                "Unconfirmed send"
            } else {
                "Unconfirmed action"
            };
            format!("F2 {title} · Enter")
        }
        ContextControl::PreviousAgent { .. } => "F2 Back · Enter".into(),
        ContextControl::Reconnect { .. } => {
            format!("F2 Reconnect · {} · Enter", Shortcut::RECONNECT.label)
        }
        ContextControl::CacheWarning { .. } => "F2 Cache warning".into(),
        ContextControl::Messages => "F2 Messages · Enter".into(),
    }
}

fn action_label(control: &ContextControl) -> &'static str {
    match control {
        ContextControl::Request { .. } => "Request",
        ContextControl::RepairCredentials { .. } => "Repair credentials",
        ContextControl::Continue { .. } => "Continue",
        ContextControl::Interrupt { .. } => "Stop",
        control @ ContextControl::PendingRecovery { .. } => {
            if control.is_send_recovery() {
                "Unconfirmed send"
            } else {
                "Unconfirmed action"
            }
        }
        ContextControl::PreviousAgent { .. } => "Previous Agent",
        ContextControl::Reconnect { .. } => "Reconnect",
        ContextControl::CacheWarning { .. } => "Cache warning",
        ContextControl::Messages => "Messages",
    }
}

pub(super) fn reading_line(app: &App, width: usize) -> Option<Line<'static>> {
    let unseen = app.pane().transcript.history_hint()?;
    let back = Shortcut::READING_BACK.label;
    let focus = if app.composer_focused() {
        "Editing"
    } else {
        "↑↓ scroll"
    };
    let content = if unseen { " · New content" } else { "" };
    let normal = format!("Reading history · {focus} · Jump to latest ({back}){content}");
    let compact = if unseen {
        format!("New · {back}")
    } else {
        format!("History · {back}")
    };
    let text = if normal.width() <= width {
        normal
    } else if compact.width() <= width {
        compact
    } else {
        back.to_owned()
    };
    let mut line = Line::styled(shorten(&text, width), Style::new().fg(MUTED));
    if let Some(rest) = text.strip_prefix("Reading history") {
        line = Line::from(vec![
            Span::styled("Reading history", emphasis(ACCENT)),
            Span::styled(rest.to_owned(), Style::new().fg(MUTED)),
        ]);
    }
    Some(line)
}

pub(super) fn focused_hint(app: &App, width: usize) -> Option<Vec<Line<'static>>> {
    if app.pane().transcript.focused_rewind_group().is_some() {
        return Some(vec![
            Line::styled(shorten("Rewound · read-only", width), focused_row()),
            Line::styled(
                key_hint(
                    "↑↓/PgUp/PgDn browse · Esc input",
                    "Esc input · ↑↓ browse",
                    width,
                ),
                Style::new().fg(MUTED),
            ),
        ]);
    }
    if !app.pane().contextual_focus.active {
        return None;
    }
    if let (Some(ContextControl::CacheWarning { since_turn }), Some(warning)) = (
        app.pane().contextual_focus.cursor.as_ref(),
        app.cache_warning(),
    ) {
        if *since_turn == warning.since_turn {
            let detail = format!(
                "Cache · Last 2 Turns · {} of {} input tokens read",
                compact_token_count(warning.read_tokens),
                compact_token_count(warning.input_tokens)
            );
            return Some(vec![
                Line::styled(shorten(&detail, width), Style::new().fg(WARNING)),
                Line::styled(shorten("Enter Dismiss · Esc back", width), focused_row()),
            ]);
        }
    }
    let action = app
        .pane()
        .contextual_focus
        .cursor
        .as_ref()
        .map_or("Context action", action_label);
    let escape = if app.escape_interrupts_running() {
        key_hint("Enter act · Esc stop", "Esc stop · Enter", width)
    } else {
        key_hint("Enter act · Esc back", "Esc back · Enter", width)
    };
    Some(vec![
        Line::styled(shorten(&format!("▶ {action}"), width), focused_row()),
        Line::styled(escape, Style::new().fg(MUTED)),
    ])
}

pub(super) fn cache_warning_hit(app: &mut App, status: Rect, rows: &[Line<'_>], focused: bool) {
    let Some(since_turn) = app.cache_warning().map(|warning| warning.since_turn) else {
        return;
    };
    let control = ContextControl::CacheWarning { since_turn };
    for (row, line) in rows.iter().take(usize::from(status.height)).enumerate() {
        let mut column = 0;
        for span in &line.spans {
            let width = span.width();
            if (!focused && span.content.starts_with("⚠ Cache"))
                || (focused && span.content.starts_with("Enter Dismiss"))
            {
                app.contextual_hits.push(ContextHit {
                    area: Rect::new(
                        status.x + column as u16,
                        status.y + row as u16,
                        width as u16,
                        1,
                    ),
                    control,
                });
                return;
            }
            column += width;
        }
    }
}

pub(super) fn recovery_title(control: &ContextControl) -> Option<&'static str> {
    match control {
        ContextControl::PendingRecovery { .. } if control.is_send_recovery() => {
            Some("Unconfirmed send")
        }
        ContextControl::PendingRecovery { .. } => Some("Unconfirmed action"),
        _ => None,
    }
}

pub(super) fn render_chip(
    frame: &mut Frame<'_>,
    app: &mut App,
    control: &ContextControl,
    area: Rect,
) {
    if area.is_empty() {
        return;
    }
    let text = shorten(&label(control), usize::from(area.width));
    let focused = app.pane().contextual_focus.active
        && app.pane().contextual_focus.cursor.as_ref() == Some(control);
    let style = if focused {
        focused_row()
    } else {
        emphasis(ACCENT)
    };
    frame.render_widget(Paragraph::new(Span::styled(text, style)), area);
    app.contextual_hits.push(ContextHit {
        area,
        control: control.clone(),
    });
}

pub(super) fn service_controls(app: &App) -> Vec<ContextControl> {
    app.contextual_controls()
        .into_iter()
        .filter(ContextControl::is_service)
        .collect()
}

pub(super) fn service_row(footer: &[Line<'_>], app: &App) -> Option<(usize, u16)> {
    let status = app.service_status().label().to_ascii_lowercase();
    footer
        .iter()
        .enumerate()
        .find(|(_, line)| {
            line.spans
                .iter()
                .any(|span| span.content.to_ascii_lowercase().contains(&status))
        })
        .map(|(row, line)| (row, u16::try_from(line.width()).unwrap_or(u16::MAX)))
}

pub(super) fn render_service(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let controls = service_controls(app);
    if area.is_empty() {
        return;
    }
    let mut x = area.x;
    for control in controls {
        let remaining = area.right().saturating_sub(x);
        if remaining == 0 {
            break;
        }
        let width = u16::try_from(label(&control).width())
            .unwrap_or(u16::MAX)
            .min(remaining);
        render_chip(
            frame,
            app,
            &control,
            Rect::new(x, area.y, width, area.height),
        );
        x = x.saturating_add(width).saturating_add(1);
    }
}

fn key_hint(full: &str, compact: &str, width: usize) -> String {
    if UnicodeWidthStr::width(full) <= width {
        full.to_owned()
    } else {
        shorten(compact, width)
    }
}
