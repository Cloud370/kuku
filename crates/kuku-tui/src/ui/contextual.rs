use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::contextual::{ContextControl, ContextHit};
use crate::keys::Shortcut;
use crate::text::shorten;
use crate::theme::{emphasis, focused_row, ACCENT};

pub(super) fn label(control: &ContextControl) -> String {
    match control {
        ContextControl::Request { .. } => "F2 Request".into(),
        ContextControl::Continue { .. } => "F2 Continue".into(),
        ContextControl::Interrupt { .. } => format!("{} Stop", Shortcut::ESCAPE.label),
        ContextControl::PendingRecovery { .. } => "F2 Recovery".into(),
        ContextControl::PreviousAgent { .. } => "F2 Back".into(),
        ContextControl::Reconnect { .. } => {
            format!("F2 Reconnect · {}", Shortcut::RECONNECT.label)
        }
        ContextControl::Restart { .. } => "F2 Restart".into(),
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
    let status = app.service_status().label();
    footer
        .iter()
        .enumerate()
        .find(|(_, line)| line.spans.iter().any(|span| span.content.contains(status)))
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
