use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Modal, ThinkingPicker};
use crate::keys::Shortcut;
use crate::menu::{Choice, ChoiceKind, CommandMenu, Picker, PickerKind};
use crate::text::{effort_label, shorten, with_suffix};
use crate::theme::{ACCENT, ERROR, MUTED, compact_padding, emphasis, focused_row, surface};

use super::{picker_line, popup, render_editor, render_popup};

const MAX_WIDTH: u16 = 76;
const MAX_ITEMS: u16 = 7;
const COMMAND_HELP: &str = "↑↓ Move · Tab Complete · Enter Run · Esc Close";
const THINKING_HELP: &str = "↑↓ Move · Enter Apply · Esc Close";
const SAVE_HELP: &str = "↑↓ Move · Enter Apply · D Save default · Esc Close";
const MODEL_HELP: &str = "↑↓ Move · Enter Select · Esc Close";

pub(super) fn render(
    frame: &mut Frame<'_>,
    editor: Rect,
    text_x: u16,
    app: &mut App,
) -> Option<Rect> {
    let modal = app.modal.as_mut()?;
    let height = match modal {
        Modal::ExitConfirmation => 1,
        Modal::SendRouteConfirmation(review) if review.shows_connections() => 4,
        Modal::SendRouteConfirmation(_) => 3,
        Modal::Picker(picker) => match &picker.thinking {
            Some(thinking) if thinking.confirm.is_some() => 3,
            thinking => {
                let items = picker
                    .visible_choices()
                    .len()
                    .clamp(1, usize::from(MAX_ITEMS)) as u16;
                let extra = u16::from(
                    thinking
                        .as_ref()
                        .is_some_and(|thinking| thinking.declared.is_some()),
                ) + picker.header.lines().count().min(3) as u16;
                items + 4 + extra
            }
        },
        _ => unreachable!("the modal is anchored to the composer"),
    };
    let height = height.min(editor.y.saturating_sub(frame.area().y));
    let area = Rect::new(editor.x, editor.y - height, editor.width, height);
    let max_width = match modal {
        Modal::Picker(picker) if picker.kind == PickerKind::Models => area.width,
        _ => MAX_WIDTH,
    };
    let inner = render_panel(frame, area, text_x, max_width);
    match modal {
        Modal::ExitConfirmation => {
            let line = Line::styled("Exit kuku? Y exit · Esc edit", emphasis(ACCENT));
            frame.render_widget(Paragraph::new(line), inner);
        }
        Modal::SendRouteConfirmation(review) => {
            let width = usize::from(inner.width);
            let show_connections = review.shows_connections();
            let label_width = if show_connections {
                width.saturating_sub(2)
            } else {
                width.saturating_sub(3) / 2
            };
            let [previous, next] = review.change_labels().map(|label| {
                crate::text::shorten_middle(&crate::text::single_line(&label), label_width)
            });
            let mut lines = vec![Line::styled(
                shorten(review.title(), width),
                emphasis(crate::theme::WARNING),
            )];
            if show_connections {
                review.labels_visible =
                    inner.height >= 4 && review.has_distinct_labels() && previous != next;
                if inner.height >= 4 {
                    lines.push(Line::raw(format!("← {previous}")));
                    lines.push(Line::raw(format!("→ {next}")));
                }
            } else if inner.height >= 3 {
                lines.push(Line::raw(format!("{previous} → {next}")));
            }
            lines.push(Line::styled(
                shorten(review.send_hint(), width),
                Style::new().fg(MUTED),
            ));
            frame.render_widget(Paragraph::new(lines), inner);
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
                let (page, width) = render_choices(frame, inner, picker, app.remote);
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
    let area = popup(area, 55, 22);
    let inner = render_popup(frame, area, "Help and shortcuts");
    let ask = if inner.width >= 24 {
        "Ask “How do I use kuku?”"
    } else {
        "Ask about kuku"
    };
    let discover = if inner.width >= 48 {
        "/ menu      Choose kuku-docs and type a question"
    } else {
        "kuku-docs in /"
    };
    let mut lines = vec![
        Line::raw(ask),
        Line::raw(discover),
        Line::raw("Enter       Send or steer"),
        Line::raw("Alt+Enter   Send on the next turn"),
        Line::raw("Esc         Close/back; stop active work"),
        Line::raw("↑ / ↓       Edit draft; select queued input"),
        Line::raw("Delete      Remove selected input from queue"),
        Line::raw("F2          Review tool approvals or reply"),
        Line::raw("F3          Change workspace permissions"),
        Line::raw(format!(
            "{:<12}Cycle thinking",
            Shortcut::CYCLE_THINKING.label
        )),
        Line::raw("Ctrl+P/F1   Show this help"),
        Line::raw("Ctrl+G      Rewind to a message"),
        Line::raw("Ctrl+V / Ctrl+Shift+V  Paste when forwarded"),
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

fn render_panel(frame: &mut Frame<'_>, area: Rect, text_x: u16, max_width: u16) -> Rect {
    let indent = text_x.saturating_sub(area.x).min(area.width);
    let mut padding = compact_padding(2, area.height >= 5);
    let mut block = Block::new().style(surface());
    let border = area.width >= 4 && indent > 0;
    if border {
        block = block
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(ACCENT));
    }
    padding.left = indent - u16::from(border);
    let block = block.padding(padding);
    frame.render_widget(Clear, area);
    let mut inner = block.inner(area);
    inner.width = inner.width.min(max_width);
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

fn render_choices(
    frame: &mut Frame<'_>,
    area: Rect,
    picker: &mut Picker,
    remote: bool,
) -> (usize, usize) {
    let commands = picker.kind == PickerKind::Commands;
    let rows = area.height;
    let header_rows = u16::from(rows >= 2);
    let footer_rows = u16::from(rows > header_rows + 1);
    let declared = picker
        .thinking
        .as_ref()
        .and_then(|thinking| thinking.declared.as_deref());
    let details = declared.unwrap_or(&picker.header);
    let details_rows = (details.lines().count().min(3) as u16)
        .min(rows.saturating_sub(header_rows + footer_rows + 1));
    let [header, information, list, footer] = Layout::vertical([
        Constraint::Length(header_rows),
        Constraint::Length(details_rows),
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
    frame.render_widget(
        Paragraph::new(
            details
                .lines()
                .take(usize::from(details_rows))
                .map(|line| {
                    Line::styled(
                        shorten(&crate::text::single_line(line), width),
                        Style::new().fg(MUTED),
                    )
                })
                .collect::<Vec<_>>(),
        ),
        information,
    );
    let field_width = render_header(frame, header, &title_line, picker);
    let count = usize::from(list.height).max(1);
    let range = picker.composer_rows(count, picker.visible_choices().len());
    if list.height >= 2
        && let Some(countdown) = &mut picker.image_auto_send
    {
        countdown.begin(std::time::Instant::now());
    }
    let choices = picker.visible_choices();
    let model_column = if picker.kind == PickerKind::Models {
        choices
            .iter()
            .filter(|choice| matches!(choice.kind, ChoiceKind::ModelRoute { .. }))
            .map(|choice| crate::text::single_line(&choice.label).width())
            .max()
            .map(|label_width| label_width.min(width / 3).max(1))
    } else {
        None
    };
    let mut lines = Vec::new();
    for (index, choice) in choices.iter().enumerate().take(range.end).skip(range.start) {
        let active = index == picker.selected;
        lines.push(choice_line(picker, choice, width, active, model_column));
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
    let marker = u16::from(list.x > 0);
    let hit_rows = Rect::new(list.x - marker, list.y, list.width + marker, list.height);
    picker.set_rows(hit_rows, range, 1);
    let help = if commands {
        COMMAND_HELP
    } else if picker
        .thinking
        .as_ref()
        .is_some_and(|thinking| thinking.save.is_some())
    {
        SAVE_HELP
    } else if matches!(picker.kind, PickerKind::Thinking | PickerKind::CustomSpeed) {
        THINKING_HELP
    } else if picker.kind == PickerKind::Images {
        "Tab / ↑↓ Choose · Enter Send · Esc Keep draft"
    } else if picker.kind == PickerKind::Speed {
        "↑↓ Move · Tab Complete · Enter Apply · Type to add"
    } else if picker.kind == PickerKind::Models {
        if picker.search_focused {
            "Enter Switch · Esc Back to list"
        } else if remote {
            "Enter Switch · / Search"
        } else {
            "Enter Switch · d Set default · e Edit · / Search"
        }
    } else {
        MODEL_HELP
    };
    let countdown = picker
        .image_auto_send
        .as_ref()
        .and_then(|countdown| countdown.seconds(std::time::Instant::now()))
        .map(|seconds| {
            format!("Auto in {seconds}s · Tab / ↑↓ Choose · Enter Send · Esc Keep draft")
        });
    let (hint, color) = picker
        .problem
        .as_ref()
        .map_or((countdown.as_deref().unwrap_or(help), MUTED), |error| {
            (error.as_str(), ERROR)
        });
    frame.render_widget(
        Paragraph::new(shorten(hint, width)).style(Style::new().fg(color)),
        footer,
    );
    (count, field_width)
}

fn render_header(frame: &mut Frame<'_>, area: Rect, title: &str, picker: &mut Picker) -> usize {
    let searchable = picker.kind.searchable();
    let title_width = if searchable {
        let preferred = title.width() as u16 + 3;
        if area.width >= preferred + 12 {
            preferred
        } else {
            0
        }
    } else {
        area.width
    };
    let marker_width =
        u16::from(searchable && picker.kind != PickerKind::Commands && area.width >= 4) * 2;
    let [heading, marker, field] = Layout::horizontal([
        Constraint::Length(title_width),
        Constraint::Length(marker_width),
        Constraint::Min(0),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(shorten(title, usize::from(heading.width))).style(emphasis(ACCENT)),
        heading,
    );
    frame.render_widget(Paragraph::new(Span::styled("/ ", emphasis(ACCENT))), marker);
    if searchable && !field.is_empty() {
        let focused = picker.kind != PickerKind::Models || picker.search_focused;
        let placeholder = if picker.kind == PickerKind::Speed {
            "Tier name…"
        } else {
            "Search models…"
        };
        render_editor(frame, field, &mut picker.query, focused, placeholder);
    }
    usize::from(field.width).max(1)
}

fn choice_line(
    picker: &Picker,
    choice: &Choice,
    width: usize,
    active: bool,
    model_column: Option<usize>,
) -> Line<'static> {
    let model_column =
        model_column.filter(|_| matches!(choice.kind, ChoiceKind::ModelRoute { .. }));
    let mut label = crate::text::single_line(&choice.label);
    if let Some(column) = model_column {
        if label.width() > column {
            label = crate::text::shorten_middle(&label, column);
        }
    }
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
    let style = if active { focused_row() } else { surface() };
    if detail.is_empty() || label.width() + 3 >= width {
        return picker_line(&label, width, active, style);
    }
    let separator = if let Some(column) = model_column {
        format!("{} · ", " ".repeat(column.saturating_sub(label.width())))
    } else if picker.kind == PickerKind::Commands {
        " ".repeat(15usize.saturating_sub(label.width()).max(2))
    } else {
        " · ".to_owned()
    };
    let info = format!("{separator}{}", crate::text::single_line(detail));
    let info = shorten(&info, width - label.width());
    let padding = " ".repeat(width - label.width() - info.width());
    Line::from(vec![
        Span::styled(label, style),
        Span::styled(info, style.fg(MUTED)),
        Span::styled(padding, style),
    ])
}
