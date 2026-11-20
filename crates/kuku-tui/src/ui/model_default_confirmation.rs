use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::markup::{render_literal, wrap};
use crate::menu::{Choice, ChoiceKind, Picker};
use crate::model_default::SAVE_SCOPE;
use crate::text::shorten;
use crate::theme::{focused_row, panel, ERROR, MUTED};

use super::picker_line;

const MODEL_DEFAULT_CONFIRMATION_MIN_WIDTH: u16 = 60;
const MODEL_DEFAULT_CONFIRMATION_MIN_HEIGHT: u16 = 20;

pub(super) fn wrap_scope(width: usize) -> Option<Vec<Line<'static>>> {
    let style = Style::default().fg(MUTED);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in SAVE_SCOPE.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        if word_width > width {
            return None;
        }
        if !line.is_empty() && UnicodeWidthStr::width(line.as_str()) + 1 + word_width > width {
            lines.push(Line::styled(std::mem::take(&mut line), style));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(Line::styled(line, style));
    }
    Some(lines)
}

pub(super) fn render_model_default_confirmation(
    frame: &mut Frame<'_>,
    inner: Rect,
    picker: &mut Picker,
) -> (usize, usize, usize) {
    let width = usize::from(inner.width);
    let details = wrap(
        render_literal(&picker.header, Style::default().fg(MUTED)),
        width,
    );
    let scope = wrap_scope(width);
    let available_rows = usize::from(inner.height);
    let terminal_area = frame.area();
    let can_confirm = terminal_area.width >= MODEL_DEFAULT_CONFIRMATION_MIN_WIDTH
        && terminal_area.height >= MODEL_DEFAULT_CONFIRMATION_MIN_HEIGHT
        && width >= 32
        && scope
            .as_ref()
            .is_some_and(|scope| available_rows >= scope.len().saturating_add(6));
    let scope = scope.unwrap_or_default();
    let save = Choice::new(
        "Save as default",
        "Write the confirmed config change",
        ChoiceKind::ConfirmModelDefault,
    );
    let cancel = Choice::new(
        "Cancel",
        "No files are changed",
        ChoiceKind::CancelModelDefault,
    );
    let has_only_cancel = picker.choice_count() == 1
        && matches!(
            picker.selected_choice(),
            Some(ChoiceKind::CancelModelDefault)
        );
    if can_confirm && picker.choice_count() != 2 {
        picker.set_local(vec![save, cancel]);
        if has_only_cancel {
            picker.selected = 1;
        }
    } else if !can_confirm && !has_only_cancel {
        picker.set_local(vec![cancel]);
        picker.selected = 0;
    }

    let scope_rows = if can_confirm { scope.len() } else { 0 };
    let choice_rows = if can_confirm { 2 } else { 1 };
    let info_rows = if can_confirm {
        available_rows.saturating_sub(scope_rows + choice_rows + 1)
    } else {
        available_rows.saturating_sub(choice_rows + 1).min(2)
    };
    let gap_rows = available_rows.saturating_sub(info_rows + scope_rows + choice_rows + 1);
    let [information, scope_area, _gap, choices_area, footer] = Layout::vertical([
        Constraint::Length(info_rows as u16),
        Constraint::Length(scope_rows as u16),
        Constraint::Length(gap_rows as u16),
        Constraint::Length(choice_rows as u16),
        Constraint::Length(1),
    ])
    .areas(inner);

    let display_details = if can_confirm {
        details
    } else {
        wrap(
            render_literal(
                "Too small to review; resize to enable Save.",
                Style::default().fg(ERROR),
            ),
            width,
        )
    };
    let detail_rows = display_details.len().min(info_rows);
    picker.header_top = picker
        .header_top
        .min(display_details.len().saturating_sub(detail_rows));
    frame.render_widget(
        Paragraph::new(
            display_details
                .into_iter()
                .skip(picker.header_top)
                .take(detail_rows)
                .collect::<Vec<_>>(),
        ),
        information,
    );
    frame.render_widget(Paragraph::new(scope), scope_area);
    let options = picker.visible_choices();
    let mut rows = Vec::new();
    for (index, choice) in options.iter().enumerate().take(2) {
        let active = index == picker.selected;
        let style = if active { focused_row() } else { panel() };
        rows.push(picker_line(
            &format!("{}{}", if active { "› " } else { "  " }, choice.label),
            width,
            active,
            style,
        ));
    }
    frame.render_widget(Paragraph::new(rows), choices_area);
    picker.set_rows(choices_area, 0..options.len().min(2), 1);
    let footer_text = if can_confirm {
        "PgUp/PgDn details · ↑↓ select · Enter confirm · Esc cancel"
    } else {
        "Resize to review before saving · Esc cancel"
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            shorten(footer_text, width),
            Style::default().fg(MUTED),
        )),
        footer,
    );
    (width.max(1), 2, info_rows)
}
