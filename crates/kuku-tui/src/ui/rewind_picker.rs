use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::REWIND_PAGE;
use crate::app::history_jump::{HistoryJumpPrompt, Preview};
use crate::markup::render_literal;
use crate::menu::{ChoiceKind, Picker, PickerKind, Prompt, PromptKind};
use crate::text::{shorten, single_line};
use crate::theme::{
    ACCENT, ERROR, MUTED, TURN_CARD, TURN_RULE, WARNING, emphasis, focused_row, panel,
    panel_padding,
};

pub(super) const WIDTH: u16 = 100;
const TURN_WIDTH: usize = 9;
const SWITCH_WIDTH: u16 = 76;

pub(super) fn height(picker: &Picker) -> u16 {
    let visible = if picker.loading {
        picker.choice_count().max(REWIND_PAGE)
    } else {
        picker.choice_count()
    };
    12 + visible.clamp(1, REWIND_PAGE) as u16
}

pub(super) fn render(
    frame: &mut Frame<'_>,
    inner: Rect,
    picker: &mut Picker,
) -> (usize, usize, usize) {
    let width = inner.width as usize;
    let all_turns = picker.kind == PickerKind::HistoryJump;
    let overview = u16::from(inner.height >= 7);
    let preview_rows = if inner.height >= 12 {
        5
    } else if inner.height >= 8 {
        3
    } else {
        0
    };
    let [heading, list, _gap, preview, status, footer] = Layout::vertical([
        Constraint::Length(overview),
        Constraint::Min(1),
        Constraint::Length(u16::from(preview_rows == 5)),
        Constraint::Length(preview_rows),
        Constraint::Length(u16::from(inner.height >= 4)),
        Constraint::Length(1),
    ])
    .areas(inner);
    let muted = Style::new().fg(MUTED);
    frame.render_widget(
        Paragraph::new(message_row(
            if all_turns {
                "  Branch    Message"
            } else {
                "  Message"
            },
            "Turn",
            width,
            muted,
            muted,
        )),
        heading,
    );

    let count = list.height as usize;
    let len = picker.choice_count();
    let visible = picker.composer_rows(count, len);
    let choices = picker.visible_choices();
    let mut rows = Vec::new();
    for (index, choice) in choices
        .iter()
        .enumerate()
        .skip(visible.start)
        .take(visible.len())
    {
        let selected = index == picker.selected;
        let style = if selected { focused_row() } else { panel() };
        let prefix = if selected { "› " } else { "  " };
        let (message, turn) = match &choice.kind {
            ChoiceKind::RewindMessage { turn, .. } => (choice.detail.clone(), turn.to_string()),
            ChoiceKind::HistoryJumpReview(destination) => {
                let branch = if destination.current_branch {
                    "Current"
                } else {
                    "Saved"
                };
                (
                    format!("{branch:8}  {}", choice.detail),
                    destination.point.number.to_string(),
                )
            }
            ChoiceKind::HistoryJumpLatest => (choice.label.clone(), String::new()),
            _ => continue,
        };
        rows.push(message_row(
            &format!("{prefix}{message}"),
            &turn,
            width,
            style,
            style.fg(if selected { ACCENT } else { MUTED }),
        ));
    }
    if rows.is_empty() {
        let placeholder = if all_turns {
            if picker.loading {
                "Loading turns…"
            } else {
                "No finished Turns"
            }
        } else if picker.loading {
            "Loading messages…"
        } else {
            "No messages to rewind"
        };
        rows.push(Line::styled(shorten(placeholder, width), muted));
    }
    frame.render_widget(Paragraph::new(rows), list);
    if preview_rows > 0 {
        match choices.get(picker.selected).map(|choice| &choice.kind) {
            Some(ChoiceKind::RewindMessage { turn, text, .. }) => {
                render_preview(frame, preview, *turn, text);
            }
            Some(ChoiceKind::HistoryJumpReview(destination)) => {
                if let Some(text) = &destination.point.preview {
                    render_preview(frame, preview, destination.point.number, text);
                }
            }
            _ => {}
        }
    }
    let show_hint = picker.loading
        || picker.rewind_history.as_ref().is_some_and(|history| {
            history.window_limited || (picker.selected == 0 && history.next_before.is_some())
        });
    if let Some((message, style)) = picker
        .problem
        .as_deref()
        .map(|error| (error, Style::new().fg(ERROR)))
        .or_else(|| show_hint.then_some((picker.header.as_str(), muted)))
    {
        frame.render_widget(
            Paragraph::new(Line::styled(shorten(&single_line(message), width), style)),
            status,
        );
    }
    frame.render_widget(Paragraph::new(controls(width)), footer);
    picker.set_rows(list, visible, 1);
    (width.max(1), count.max(1), 0)
}

fn render_preview(frame: &mut Frame<'_>, preview: Rect, turn: u64, text: &str) {
    let body = message_card(frame, preview, &format!("Preview  ·  Turn {turn}"));
    let end = text
        .char_indices()
        .nth(512)
        .map_or(text.len(), |(end, _)| end);
    frame.render_widget(message_paragraph(&text[..end]), body);
}

fn message_card(frame: &mut Frame<'_>, area: Rect, heading: &str) -> Rect {
    let card = Block::new()
        .style(panel().bg(TURN_CARD))
        .borders(Borders::LEFT)
        .border_style(Style::new().fg(TURN_RULE))
        .padding(Padding::new(1, 1, 0, 0));
    let content = card.inner(area);
    frame.render_widget(card, area);
    let [title, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(content);
    frame.render_widget(
        Paragraph::new(Line::styled(
            shorten(heading, title.width as usize),
            emphasis(ACCENT),
        )),
        title,
    );
    body
}

fn message_paragraph(text: &str) -> Paragraph<'static> {
    Paragraph::new(render_literal(text, panel().bg(TURN_CARD))).wrap(Wrap { trim: true })
}

pub(super) fn render_history_jump(
    frame: &mut Frame<'_>,
    area: Rect,
    prompt: &mut Prompt,
) -> (Rect, usize) {
    let PromptKind::HistoryJump(review) = &prompt.kind else {
        unreachable!("history switch requires a selected Turn");
    };
    let sizing = super::popup(area, SWITCH_WIDTH, 15);
    let padding = panel_padding(sizing);
    let width = sizing
        .width
        .saturating_sub(padding.left + padding.right)
        .max(1);
    let preview = review
        .destination
        .point
        .preview
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .map(message_paragraph);
    let card_rows = preview.as_ref().map_or(0, |text| {
        text.line_count(width.saturating_sub(3).max(1)).min(2) as u16 + 1
    });
    let intro = switch_description(review);
    let intro_rows = intro.line_count(width).min(3) as u16;
    let files = file_restore_description(review, usize::from(width));
    let files_rows = files.line_count(width).min(u16::MAX as usize) as u16;
    let popup = super::popup(
        area,
        SWITCH_WIDTH,
        (card_rows + intro_rows + 4)
            .saturating_add(files_rows)
            .saturating_add(padding.top + padding.bottom),
    );
    let inner = super::render_popup(frame, popup, &prompt.title);
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let card_rows = card_rows.min(body.height.saturating_sub(intro_rows + 3));
    let [card, intro_area, _gap, files_area] = Layout::vertical([
        Constraint::Length(card_rows),
        Constraint::Length(intro_rows.min(body.height.saturating_sub(1))),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(body);
    if let Some(preview) = preview.filter(|_| card.height >= 2) {
        let body = message_card(frame, card, "Original message");
        frame.render_widget(preview, body);
    }
    frame.render_widget(intro, intro_area);
    let visible = usize::from(files_area.height).max(1);
    let can_scroll = files_rows as usize > visible;
    prompt.instructions_top = prompt
        .instructions_top
        .min((files_rows as usize).saturating_sub(visible));
    frame.render_widget(
        files.scroll((prompt.instructions_top as u16, 0)),
        files_area,
    );
    let mut keys = Vec::new();
    if review.ready() {
        keys.push("Enter Switch");
    } else if matches!(review.draft, Preview::Failed(_)) {
        keys.push("Enter Retry");
    }
    if review.can_restore_files() {
        keys.push(if review.file_restore_selected() {
            "Space Keep files"
        } else {
            "Space Preview files"
        });
    }
    if can_scroll {
        keys.push("PgUp/PgDn Files");
    }
    keys.push("Esc Back");
    frame.render_widget(
        Paragraph::new(Line::styled(
            shorten(&keys.join("   "), usize::from(inner.width)),
            Style::new().fg(MUTED),
        )),
        footer,
    );
    (inner, visible)
}

fn switch_description(review: &HistoryJumpPrompt) -> Paragraph<'static> {
    let (text, color) = match &review.draft {
        Preview::Loading(_) => ("Loading original message…".to_owned(), MUTED),
        Preview::Failed(error) => (
            format!("Could not load the original message. {error}"),
            ERROR,
        ),
        Preview::Ready(_) if review.existing_draft => {
            ("Your draft stays in the input box.".to_owned(), MUTED)
        }
        Preview::Ready(Some(_)) => (
            "The original message returns as an unsent draft.".to_owned(),
            MUTED,
        ),
        Preview::Ready(None) => (
            "Conversation and tool results through this turn.".to_owned(),
            MUTED,
        ),
    };
    let mode = if !review.destination.point.can_continue {
        "Reopen an unfinished turn"
    } else if review.destination.current_branch {
        "Current history"
    } else {
        "Saved history"
    };
    Paragraph::new(vec![
        Line::styled(mode, emphasis(ACCENT)),
        Line::styled(text, Style::new().fg(color)),
    ])
    .wrap(Wrap { trim: true })
}

fn file_restore_description(review: &HistoryJumpPrompt, width: usize) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if review.can_restore_files() {
        let (label, accent) = if review.file_restore_selected() {
            ("[x] Restore files changed after this turn", WARNING)
        } else {
            ("[ ] Restore files changed after this turn", ACCENT)
        };
        lines.push(Line::styled(label, emphasis(accent)));
        lines.push(Line::styled(
            "Built-in write_file and edit_file only.",
            Style::new().fg(MUTED),
        ));
        lines.push(Line::styled(
            "Script and shell changes stay as they are.",
            Style::new().fg(MUTED),
        ));
    } else {
        lines.push(Line::styled("Keep current files", emphasis(ACCENT)));
    }
    match &review.files {
        Some(Preview::Loading(_)) => lines.push(Line::styled(
            "Checking file versions…",
            Style::new().fg(MUTED),
        )),
        Some(Preview::Failed(error)) => lines.push(Line::styled(
            format!("{error} · Space to keep files"),
            Style::new().fg(ERROR),
        )),
        Some(Preview::Ready(plan)) => {
            let title = if plan.changes.is_empty() {
                "No files to restore. Current files will stay as they are.".to_owned()
            } else {
                format!(
                    "{} {} to restore",
                    plan.changes.len(),
                    if plan.changes.len() == 1 {
                        "file"
                    } else {
                        "files"
                    }
                )
            };
            lines.extend([Line::default(), Line::styled(title, emphasis(ACCENT))]);
            for change in &plan.changes {
                let action = if change.remove { "Remove" } else { "Restore" };
                let count = change.lines.map_or_else(
                    || "line count unavailable".to_owned(),
                    |(added, removed)| format!("+{added} −{removed}"),
                );
                lines.push(file_row(
                    &change.path,
                    &format!("{action}   {count}"),
                    width,
                    WARNING,
                ));
            }
            if !plan.skipped.is_empty() {
                lines.push(Line::styled(
                    "Skipped · file version could not be verified",
                    emphasis(MUTED),
                ));
                for path in &plan.skipped {
                    lines.push(file_row(path, "", width, MUTED));
                }
            }
        }
        None => {}
    }
    Paragraph::new(lines).wrap(Wrap { trim: true })
}

fn file_row(path: &str, status: &str, width: usize, color: ratatui::style::Color) -> Line<'static> {
    let status_width = status.width();
    let path_width = width.saturating_sub(status_width + 4);
    Line::from(vec![
        Span::raw("  "),
        Span::raw(shorten(&single_line(path), path_width)),
        Span::styled(
            if status.is_empty() {
                String::new()
            } else {
                format!("  {status}")
            },
            Style::new().fg(color),
        ),
    ])
}

pub(super) fn render_review(
    frame: &mut Frame<'_>,
    inner: Rect,
    prompt: &mut Prompt,
) -> (usize, usize) {
    let PromptKind::RewindMessage {
        text,
        restore_files,
        ..
    } = &prompt.kind
    else {
        unreachable!("rewind confirmation requires a selected message");
    };
    let restore_files = *restore_files;
    let spacious = inner.height >= 13;
    let gaps = u16::from(spacious);
    let [effect, _before, message, _after, files, footer] = Layout::vertical([
        Constraint::Length(if spacious { 3 } else { 2 }),
        Constraint::Length(gaps),
        Constraint::Min(0),
        Constraint::Length(gaps),
        Constraint::Length(if spacious { 4 } else { 2 }),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("Conversation", emphasis(ACCENT)),
            Line::styled(
                "This message and later activity leave this Agent’s active history.",
                panel(),
            ),
            Line::styled(
                "The original conversation is saved. The message returns as an unsent draft.",
                Style::new().fg(MUTED),
            ),
        ])
        .wrap(Wrap { trim: true }),
        effect,
    );

    let body = message_card(frame, message, "Selected message");
    let paragraph = message_paragraph(text);
    let message_rows = paragraph.line_count(body.width);
    let visible = body.height as usize;
    let top = message_rows.saturating_sub(visible).min(u16::MAX as usize);
    prompt.instructions_top = prompt.instructions_top.min(top);
    frame.render_widget(paragraph.scroll((prompt.instructions_top as u16, 0)), body);

    let files_title = if restore_files {
        "Files   Restore from journal"
    } else {
        "Files   Keep current files"
    };
    let files_style = if restore_files {
        emphasis(WARNING)
    } else {
        emphasis(ACCENT)
    };
    let file_lines = if restore_files {
        [
            "Restore earlier contents and remove files recorded as created after this message.",
            "Only built-in write_file and edit_file. Script and shell changes stay.",
        ]
    } else {
        [
            "Workspace files stay as they are.",
            "Restore mode can replace file contents or remove files created after this message.",
        ]
    };
    let mut file_text = vec![Line::styled(files_title, files_style)];
    file_text.extend(file_lines.map(|line| Line::styled(line, Style::new().fg(MUTED))));
    frame.render_widget(Paragraph::new(file_text).wrap(Wrap { trim: true }), files);

    let action = if restore_files {
        " Edit and restore files   "
    } else {
        " Edit this message   "
    };
    let keys = emphasis(ACCENT);
    let muted = Style::new().fg(MUTED);
    let mut controls = vec![
        Span::styled("Enter", keys),
        Span::styled(action, muted),
        Span::styled("Space", keys),
        Span::styled(" Change files mode   ", muted),
        Span::styled("Esc", keys),
        Span::styled(" Cancel", muted),
    ];
    if top != 0 && usize::from(footer.width) >= 81 {
        controls.extend([
            Span::styled("   PgUp/PgDn", keys),
            Span::styled(" Message", muted),
        ]);
    }
    frame.render_widget(Paragraph::new(Line::from(controls)), footer);
    (body.width as usize, visible)
}

fn controls(width: usize) -> Line<'static> {
    let keys = emphasis(ACCENT);
    let muted = Style::new().fg(MUTED);
    if width < "↑↓ Select   Enter Review   Esc Close".width() {
        return Line::styled(shorten("↑↓  Enter Review  Esc", width), muted);
    }
    Line::from(vec![
        Span::styled("↑↓", keys),
        Span::styled(" Select   ", muted),
        Span::styled("Enter", keys),
        Span::styled(" Review   ", muted),
        Span::styled("Esc", keys),
        Span::styled(" Close", muted),
    ])
}

fn message_row(
    message: &str,
    turn: &str,
    width: usize,
    style: Style,
    turn_style: Style,
) -> Line<'static> {
    if width <= TURN_WIDTH + 8 {
        return Line::styled(shorten(message, width), style);
    }
    let message_width = width - TURN_WIDTH;
    let message = shorten(message, message_width);
    let padding = " ".repeat(message_width.saturating_sub(message.width()));
    let turn = shorten(turn, TURN_WIDTH - 1);
    Line::from(vec![
        Span::styled(format!("{message}{padding}"), style),
        Span::styled(format!("{:>8} ", turn), turn_style),
    ])
}
