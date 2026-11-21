use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Modal, ThinkingPicker};
use crate::keys::Shortcut;
use crate::menu::{Choice, ChoiceKind, CommandMenu, Picker, PickerKind};
use crate::text::{effort_label, shorten, with_suffix};
use crate::theme::{emphasis, focused_row, panel, ACCENT, ERROR, MUTED};

use super::{picker_line, popup, render_editor, render_popup};

const MAX_WIDTH: u16 = 64;
const MAX_ITEMS: u16 = 7;
const COMMAND_HELP: &str = "↑↓ Move · Tab Complete · Enter Run · Esc Close";
const THINKING_HELP: &str = "↑↓ Move · Enter Apply · Esc Close";
const SAVE_HELP: &str = "↑↓ Move · Enter Apply · D Save default · Esc Close";

pub(super) fn render(
    frame: &mut Frame<'_>,
    editor: Rect,
    text_x: u16,
    app: &mut App,
) -> Option<Rect> {
    let modal = app.modal.as_mut()?;
    let height = match modal {
        Modal::ExitConfirmation => 1,
        Modal::Picker(picker) => match &picker.thinking {
            Some(thinking) if thinking.confirm.is_some() => 3,
            thinking => {
                let items = picker
                    .visible_choices()
                    .len()
                    .clamp(1, usize::from(MAX_ITEMS)) as u16;
                let extra = picker.kind == PickerKind::Commands
                    || thinking
                        .as_ref()
                        .is_some_and(|thinking| thinking.declared.is_some());
                items + 2 + u16::from(extra)
            }
        },
        _ => unreachable!("the modal is anchored to the composer"),
    };
    let height = height.min(editor.y.saturating_sub(frame.area().y));
    let area = Rect::new(
        editor.x,
        editor.y - height,
        editor.width.min(MAX_WIDTH),
        height,
    );
    let inner = render_panel(frame, area, text_x);
    match modal {
        Modal::ExitConfirmation => {
            let line = Line::styled("Exit kuku? Y exit · Esc edit", emphasis(ACCENT));
            frame.render_widget(Paragraph::new(line), inner);
        }
        Modal::Picker(picker) => {
            let confirmation = picker
                .thinking
                .as_ref()
                .and_then(|thinking| thinking.confirm.as_deref().map(|effort| (thinking, effort)));
            if let Some((thinking, effort)) = confirmation {
                render_confirmation(frame, inner, thinking, effort);
                picker.set_rows(Rect::ZERO, 0..0, 1);
            } else {
                let (page, width) = render_choices(frame, inner, picker);
                app.modal_page = page;
                app.modal_editor_width = width;
            }
        }
        _ => unreachable!("the modal is anchored to the composer"),
    }
    Some(area)
}

pub(super) fn render_alias_hint(
    frame: &mut Frame<'_>,
    gap: Rect,
    editor: Rect,
    text_x: u16,
    app: &App,
) {
    let Some(hint) = &app.alias_hint else {
        return;
    };
    let width = usize::from(editor.right().saturating_sub(text_x));
    let text = shorten(&hint.text, width);
    let y = if gap.is_empty() { editor.y } else { gap.y };
    let area = Rect::new(text_x, y, text.width() as u16, 1).intersection(frame.area());
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(MUTED)), area);
}

pub(super) fn render_help(frame: &mut Frame<'_>, area: Rect) -> Rect {
    let area = popup(area, 55, 20);
    let inner = render_popup(frame, area, "Keyboard shortcuts");
    let mut lines = vec![
        Line::raw("Enter       Send or steer"),
        Line::raw("Alt+Enter   Send on the next turn"),
        Line::raw("Esc         Close/back; stop active work"),
        Line::raw("↑ / ↓       Edit draft; select queued input"),
        Line::raw("Delete      Withdraw selected queued input"),
        Line::raw("F2          Focus a pending request"),
        Line::raw(format!(
            "{:<12}Cycle thinking",
            Shortcut::CYCLE_THINKING.label
        )),
        Line::raw("Ctrl+P/F1   Show this help"),
        Line::raw("/           Search actions and Skills"),
        Line::default(),
    ];
    lines.extend(CommandMenu::ALL.chunks(4).map(|commands| {
        let labels = commands
            .iter()
            .map(|command| format!("/{}", command.name()))
            .collect::<Vec<_>>();
        Line::raw(labels.join("  "))
    }));
    frame.render_widget(Paragraph::new(lines).style(Style::new().fg(MUTED)), inner);
    inner
}

fn render_panel(frame: &mut Frame<'_>, area: Rect, text_x: u16) -> Rect {
    let indent = text_x.saturating_sub(area.x).min(area.width);
    let border = if indent > 0 {
        Borders::LEFT
    } else {
        Borders::NONE
    };
    let block = Block::new()
        .style(panel())
        .borders(border)
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::new(indent.saturating_sub(1), 1, 0, 0));
    frame.render_widget(Clear, area);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn render_confirmation(frame: &mut Frame<'_>, area: Rect, thinking: &ThinkingPicker, effort: &str) {
    let width = usize::from(area.width);
    let level = effort_label(Some(effort));
    let lines = vec![
        Line::styled(
            shorten(&format!("Save {level} as model default?"), width),
            emphasis(ACCENT),
        ),
        Line::raw(crate::text::shorten_middle(
            &crate::text::single_line(&thinking.route.model),
            width,
        )),
        Line::styled("Enter Save · Esc Back", Style::new().fg(MUTED)),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_choices(frame: &mut Frame<'_>, area: Rect, picker: &mut Picker) -> (usize, usize) {
    let commands = picker.kind == PickerKind::Commands;
    let rows = area.height;
    let title_rows = u16::from(rows >= 3);
    let query_rows = u16::from(commands && rows >= 2);
    let footer_rows = u16::from(rows > title_rows + query_rows + 1);
    let declared = picker
        .thinking
        .as_ref()
        .and_then(|thinking| thinking.declared.as_deref());
    let declared_rows = u16::from(declared.is_some() && rows > title_rows + footer_rows + 1);
    let [title, query, declaration, list, footer] = Layout::vertical([
        Constraint::Length(title_rows),
        Constraint::Length(query_rows),
        Constraint::Length(declared_rows),
        Constraint::Min(0),
        Constraint::Length(footer_rows),
    ])
    .areas(area);
    let width = usize::from(area.width);
    let title_line = match &picker.thinking {
        Some(thinking) => {
            let model = format!(
                "Thinking · {}",
                crate::text::single_line(&thinking.route.model)
            );
            let scope = if thinking.pending {
                "Next request"
            } else {
                "Current"
            };
            if let Some(level) = &thinking.unlisted_current {
                with_suffix(&model, &format!("{scope} · {level}"), width)
            } else if thinking.pending {
                with_suffix(&model, scope, width)
            } else {
                shorten(&model, width)
            }
        }
        None => picker.kind.title().into(),
    };
    frame.render_widget(Paragraph::new(title_line).style(emphasis(ACCENT)), title);
    if let Some(declared) = declared {
        frame.render_widget(
            Paragraph::new(shorten(declared, width)).style(Style::new().fg(MUTED)),
            declaration,
        );
    }
    let field_width = render_query(frame, query, picker);
    let count = usize::from(list.height).max(1);
    let range = picker.composer_rows(count, picker.visible_choices().len());
    let choices = picker.visible_choices();
    let mut lines = Vec::new();
    for (index, choice) in choices.iter().enumerate().take(range.end).skip(range.start) {
        let active = index == picker.selected;
        lines.push(choice_line(picker, choice, width, active));
        if active && list.x > 0 {
            let cursor = Rect::new(list.x - 1, list.y + (index - range.start) as u16, 1, 1);
            frame.render_widget(
                Paragraph::new("›").style(focused_row().patch(emphasis(ACCENT))),
                cursor,
            );
        }
    }
    if choices.is_empty() {
        let status = if picker.loading {
            "Loading…"
        } else {
            "No matching entries"
        };
        lines.push(Line::styled(status, Style::new().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(lines), list);
    picker.set_rows(list, range, 1);
    let help = if commands {
        COMMAND_HELP
    } else if picker
        .thinking
        .as_ref()
        .is_some_and(|thinking| thinking.save.is_some())
    {
        SAVE_HELP
    } else {
        THINKING_HELP
    };
    let (hint, color) = picker
        .problem
        .as_ref()
        .map_or((help, MUTED), |error| (error.as_str(), ERROR));
    frame.render_widget(
        Paragraph::new(shorten(hint, width)).style(Style::new().fg(color)),
        footer,
    );
    (count, field_width)
}

fn render_query(frame: &mut Frame<'_>, area: Rect, picker: &mut Picker) -> usize {
    if area.is_empty() {
        return usize::from(area.width).max(1);
    }
    let marker_width = u16::from(!picker.query.text().starts_with('/'));
    let [marker, field] =
        Layout::horizontal([Constraint::Length(marker_width), Constraint::Min(0)]).areas(area);
    frame.render_widget(Paragraph::new(Span::styled("/", emphasis(ACCENT))), marker);
    render_editor(frame, field, &mut picker.query, true, "");
    usize::from(field.width).max(1)
}

fn choice_line(picker: &Picker, choice: &Choice, width: usize, active: bool) -> Line<'static> {
    let mut label = crate::text::single_line(&choice.label);
    let mut detail = choice.detail.as_str();
    let inherited;
    if let (Some(thinking), ChoiceKind::Thinking(effort)) = (&picker.thinking, &choice.kind) {
        if *effort == thinking.current {
            label.push_str(" ✓");
        }
        if effort.is_none() {
            let setting = thinking.inherited.as_deref().unwrap_or(if picker.loading {
                "Loading…"
            } else {
                "Unavailable"
            });
            inherited = format!("Model default · {setting}");
            detail = &inherited;
        } else if *effort == thinking.current {
            detail = "Agent override";
        }
    }
    let style = if active { focused_row() } else { panel() };
    if detail.is_empty() || label.width() + 3 >= width {
        return picker_line(&label, width, active, style);
    }
    let info = format!(" · {}", crate::text::single_line(detail));
    let info = shorten(&info, width - label.width());
    let padding = " ".repeat(width - label.width() - info.width());
    Line::from(vec![
        Span::styled(label, style),
        Span::styled(info, style.fg(MUTED)),
        Span::styled(padding, style),
    ])
}
