use kuku_client::AgentStatus;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Modal};
use crate::editor::{Editor, EditorView};
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap};
use crate::menu::{Choice, ChoiceKind, Picker, PickerKind, Prompt, PromptKind};
use crate::model_default::SAVE_SCOPE;
use crate::output::OutputViewer;
use crate::text::{shorten, single_line};
use crate::theme::{
    composer_padding, emphasis, focused_row, horizontal_gutter, panel, panel_padding, surface,
    ACCENT, ERROR, MAX_DRAFT_ROWS, MUTED, OUTPUT_WIDTH, PICKER_HEIGHT, PICKER_WIDTH,
    SHORT_PROMPT_HEIGHT, STATUS_EDITOR_HEIGHT, WARNING,
};

mod activity;
mod delivery;
mod header;

pub(crate) use activity::{ActivityClock, ActivityFrame};

const MAX_BANNER_ROWS: u16 = 5;
const MIN_TRANSCRIPT_ROWS: u16 = 2;
const MIN_ACTIVITY_WIDTH: u16 = 8;
const FOOTER_SPACE_HEIGHT: u16 = 16;
const COMPOSER_SPACE_HEIGHT: u16 = 20;
const COMPOSER_SPACE_WIDTH: usize = 42;

#[cfg(test)]
pub(crate) fn render(frame: &mut Frame<'_>, app: &mut App) {
    let activity = ActivityFrame::test_frame(app, None);
    render_at(frame, app, activity);
}

pub(crate) fn render_at(
    frame: &mut Frame<'_>,
    app: &mut App,
    activity_frame: ActivityFrame,
) -> bool {
    let outer = frame.area();
    if outer.is_empty() {
        app.text_selection.clear();
        app.delivery_area = Rect::ZERO;
        app.delivery_hits.clear();
        app.request_area = Rect::ZERO;
        app.request_hit = None;
        app.permission_choice_areas = [Rect::ZERO; 2];
        app.reading_area = Rect::ZERO;
        app.composer_area = Rect::ZERO;
        return false;
    }
    let area = outer.inner(Margin::new(horizontal_gutter(outer.width), 0));
    let width = usize::from(area.width);
    let footer = app.displayed_statusline().render_rows(
        app,
        width,
        area.height,
        activity_frame.agent_activity(app),
    );
    let status_height = footer.len() as u16;
    let status_gap = u16::from(status_height > 0 && area.height >= FOOTER_SPACE_HEIGHT);
    let below_editor = status_height + status_gap;
    let chrome_rows = if area.height > 3 {
        MIN_TRANSCRIPT_ROWS
    } else {
        0
    };
    let padding = composer_padding(area);
    let padding_rows = padding.top + padding.bottom;
    let composer_gap = u16::from(area.height >= 16);
    let space_above_editor = area
        .height
        .saturating_sub(below_editor + chrome_rows + padding_rows + composer_gap + 2);
    let unobstructed = app.modal.is_none();
    let focused = unobstructed && app.composer_focused();
    let mut composer = Block::new().style(surface());
    if area.width >= 4 {
        composer = composer
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(if focused { ACCENT } else { MUTED }))
            .padding(padding);
    }
    let composer_content = composer.inner(area);
    let content_width = usize::from(composer_content.width);
    let hint_limit = if content_width >= 70 {
        content_width.saturating_sub(12).min(70)
    } else {
        content_width.saturating_sub(7)
    };
    let reading = reading_line(app, hint_limit);
    let hint_width = reading.as_ref().map_or(0, Line::width);
    let stats_width = content_width.saturating_sub(hint_width + usize::from(hint_width > 0));
    let dock = reading
        .as_ref()
        .and_then(|_| app.pane().transcript.turn_dock(stats_width));
    let expanded_request = app.pane().keyboard_focus == crate::app::KeyboardFocus::Request
        && app.pane().snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.agent.question.is_none() && snapshot.agent.permission.is_some()
        });
    let request = delivery::request_lines(
        app,
        width,
        space_above_editor.min(if expanded_request { 12 } else { 3 }),
    );
    let request_height = request.lines.len() as u16;
    app.pane_mut().request_detail_page_size = request.detail_page_size;
    let deliveries = delivery::lines(
        app,
        width,
        space_above_editor.saturating_sub(request_height),
    );
    let delivery_height = deliveries.lines.len() as u16;
    let above_editor = composer_gap + request_height + delivery_height;
    let editor_limit = area
        .height
        .saturating_sub(below_editor + chrome_rows + padding_rows + above_editor);
    let heading_rows = u16::from(editor_limit >= 2);
    let heading_gap = u16::from(
        editor_limit >= 3
            && area.height >= COMPOSER_SPACE_HEIGHT
            && content_width >= COMPOSER_SPACE_WIDTH,
    );
    let draft_limit = usize::from(editor_limit - heading_rows - heading_gap).min(MAX_DRAFT_ROWS);
    let draft = (draft_limit > 0).then(|| app.pane_mut().draft.project(content_width, draft_limit));
    let editor_height = padding_rows
        + heading_rows
        + heading_gap
        + draft.as_ref().map_or(0, |view| view.lines.len() as u16);
    let banner_rows = area
        .height
        .saturating_sub(editor_height + above_editor + below_editor + MIN_TRANSCRIPT_ROWS)
        .min(MAX_BANNER_ROWS);
    let banner = banner(app, width, usize::from(banner_rows));
    let [transcript, banner_area, gap, request_area, delivery_area, editor, _status_gap, status] =
        Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(banner.len() as u16),
            Constraint::Length(composer_gap),
            Constraint::Length(request_height),
            Constraint::Length(delivery_height),
            Constraint::Length(editor_height),
            Constraint::Length(status_gap),
            Constraint::Length(status_height),
        ])
        .areas(area);
    let row_insets = render_transcript(
        frame,
        transcript,
        app,
        banner_area.height == 0,
        &activity_frame,
    );
    frame.render_widget(Paragraph::new(banner), banner_area);
    let dock_area = if gap.height > 0 {
        gap
    } else {
        Rect::new(
            transcript.x,
            transcript.bottom().saturating_sub(1),
            transcript.width,
            u16::from(transcript.height > 1),
        )
    };
    let dock_inner = Rect::new(
        composer_content.x,
        dock_area.y,
        composer_content.width,
        dock_area.height,
    );
    let [stats_area, reading_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(hint_width as u16)])
            .areas(dock_inner);
    if dock.is_some() || reading.is_some() {
        let edge = if area.width >= 4 {
            Borders::LEFT
        } else {
            Borders::NONE
        };
        frame.render_widget(
            Block::new()
                .borders(edge)
                .border_style(Style::new().fg(if focused { ACCENT } else { MUTED }))
                .style(Style::new().bg(crate::theme::TURN_CARD)),
            dock_area,
        );
    }
    if let Some(line) = dock {
        frame.render_widget(Paragraph::new(line), stats_area);
    }
    if let Some(line) = reading {
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(crate::theme::TURN_CARD)),
            reading_area,
        );
    }
    frame.render_widget(Paragraph::new(request.lines), request_area);
    frame.render_widget(Paragraph::new(deliveries.lines), delivery_area);
    app.request_area = request_area;
    let request_id = app.pane().snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .agent
            .question
            .as_ref()
            .map(|request| request.id.clone())
            .or_else(|| {
                snapshot
                    .agent
                    .permission
                    .as_ref()
                    .map(|request| request.id.clone())
            })
    });
    app.request_hit = if request_area.is_empty() {
        None
    } else {
        request_id.map(|request_id| (app.selection().target, request_id))
    };
    app.delivery_area = delivery_area;
    app.delivery_hits = deliveries.hits;
    app.permission_choice_areas =
        request
            .permission_choices
            .map_or([Rect::ZERO; 2], |(row, deny, allow)| {
                [
                    delivery::choice_area(request_area, row, deny),
                    delivery::choice_area(request_area, row, allow),
                ]
            });
    app.reading_area = reading_area;
    let [heading, _heading_gap, inner] = Layout::vertical([
        Constraint::Length(heading_rows),
        Constraint::Length(heading_gap),
        Constraint::Min(0),
    ])
    .areas(composer.inner(editor));
    frame.render_widget(composer, editor);
    app.editor_width = content_width.max(1);
    app.composer_area = inner;
    let activity_visible =
        unobstructed && heading.width >= MIN_ACTIVITY_WIDTH && !heading.is_empty();
    if !heading.is_empty() {
        let state = if activity_visible {
            activity_frame
        } else {
            activity_frame.still()
        };
        let line = activity::render_heading(app, usize::from(heading.width), state);
        frame.render_widget(Paragraph::new(line), heading);
    }
    if let Some(draft) = draft {
        let placeholder = app.pane().draft.text().is_empty().then(|| {
            shorten(
                &format!(
                    "Enter steer · {} next Turn · Esc Esc Rewind · / actions · {} help",
                    Shortcut::NEXT_TURN.label,
                    Shortcut::ACTIONS.label
                ),
                content_width,
            )
        });
        render_editor_view(frame, inner, draft, focused, placeholder.as_deref());
    }
    frame.render_widget(Paragraph::new(footer), status);
    let main_areas = [
        transcript,
        banner_area,
        request_area,
        delivery_area,
        reading_area,
        inner,
        status,
    ];
    let modal_area = render_modal(frame, outer, app);
    app.text_selection.render(
        frame.buffer_mut(),
        modal_area
            .as_ref()
            .map_or(&main_areas[..], std::slice::from_ref),
        modal_area.is_none().then_some((transcript, row_insets)),
    );
    activity_visible
}

fn reading_line(app: &App, width: usize) -> Option<Line<'static>> {
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

fn render_transcript(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    show_service_problem: bool,
    activity: &ActivityFrame,
) -> Vec<u16> {
    if area.is_empty() {
        app.transcript_area = (usize::from(area.width).max(1), 1);
        return Vec::new();
    }
    let [header, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let (width, height) = (usize::from(body.width), usize::from(body.height));
    app.transcript_area = (width.max(1), height.max(1));
    let transcript = &mut app
        .panes
        .get_mut(&app.active)
        .expect("active pane")
        .transcript;
    let agents = &app.related_agents;
    transcript.sync_agent_activity(
        agents.revision(),
        agents
            .iter()
            .map(|agent| (agent.address, agent.status, agent.preview)),
    );
    let mut view = transcript.project(width, height);
    if let Some(row) = view.thinking_row {
        view.lines[row] = if app.pane().activity() == crate::app::Activity::Thinking {
            activity.thinking_line(width)
        } else {
            Line::default()
        };
    }
    if view.lines.is_empty() && !app.pane().exists {
        header::render_welcome(frame, area, app, show_service_problem);
        return Vec::new();
    }
    if view.lines.is_empty() && height > 0 {
        view.lines = header::empty_notice(app, width, show_service_problem);
    }
    header::render_session(
        frame,
        header,
        app,
        view.following,
        view.tool_hint.as_deref(),
    );
    for (row, line) in view.lines.iter().take(height).enumerate() {
        if line.style.bg.is_some() {
            let area = Rect::new(body.x, body.y + row as u16, body.width, 1);
            frame.buffer_mut().set_style(area, line.style);
        }
    }
    frame.render_widget(Paragraph::new(view.lines), body);
    view.row_insets.insert(0, 0);
    view.row_insets
}

fn banner(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    let pane = app.pane();
    let mut lines = Vec::new();
    if let Some(retry) = pane.retry_metadata() {
        let color = match crate::retry_notice::decision(retry) {
            Some(false) => ERROR,
            Some(true) | None => WARNING,
        };
        lines.push(Line::styled(
            crate::text::shorten_middle(
                &crate::retry_notice::banner(retry, time::OffsetDateTime::now_utc()),
                width,
            ),
            emphasis(color),
        ));
        let detail_rows = height.saturating_sub(lines.len()).min(2);
        if detail_rows > 0 {
            lines.extend(crate::markup::wrap_bounded(
                vec![Line::styled(
                    crate::retry_notice::detail(retry),
                    emphasis(color),
                )],
                width,
                detail_rows,
            ));
        }
    }
    lines.extend(waiting(app, width));
    if let Some(problem) = app.service_problem() {
        lines.extend(
            problem
                .lines()
                .take(2)
                .map(|line| Line::styled(shorten(&single_line(line), width), emphasis(WARNING))),
        );
    }
    if let Some(problem) = pane.model_problem.as_ref() {
        lines.push(Line::styled(
            shorten(
                &format!("Server configuration · /model · {}", single_line(problem)),
                width,
            ),
            emphasis(WARNING),
        ));
    } else if let Some(next) = pane.next_model_label() {
        lines.push(Line::styled(
            shorten(
                &format!("Next model request · {}", single_line(&next)),
                width,
            ),
            Style::default().fg(ACCENT),
        ));
    }
    if let Some(snapshot) = &pane.snapshot {
        if let Some(error) = &snapshot.agent.last_error {
            let recovery = if snapshot.agent.status == AgentStatus::Interrupted {
                format!(
                    "Interrupted · /continue after reviewing tool effects · {}",
                    single_line(error)
                )
            } else {
                format!("Execution failed · {}", single_line(error))
            };
            lines.push(Line::styled(
                shorten(&recovery, width),
                Style::default().fg(ERROR),
            ));
        }
    }
    if let Some(notice) = app.notice.as_ref().filter(|notice| !notice.transient()) {
        let color = if notice.error { ERROR } else { ACCENT };
        lines.push(Line::styled(
            shorten(&single_line(&notice.text), width),
            Style::default().fg(color),
        ));
    }
    lines.truncate(height);
    lines
}

fn waiting(app: &App, width: usize) -> Vec<Line<'static>> {
    let Some(snapshot) = &app.pane().snapshot else {
        return Vec::new();
    };
    let (action, request) = if let Some(question) = &snapshot.agent.question {
        ("Reply", &question.text)
    } else if let Some(permission) = &snapshot.agent.permission {
        ("Review authorization", &permission.summary)
    } else {
        return Vec::new();
    };
    let agent = single_line(&app.active.agent);
    let compact = if app.available() {
        format!("{} {action}", Shortcut::REPLY.label)
    } else {
        "Last seen waiting".into()
    };
    let title = if app.available() {
        format!("Waiting · {agent} · {compact}")
    } else {
        format!("{compact} · {agent}")
    };
    let title = if title.width() <= width {
        &title
    } else {
        &compact
    };
    let style = emphasis(WARNING);
    let mut lines = vec![Line::styled(shorten(title, width), style)];
    lines.extend(
        wrap(render_literal(request, style), width)
            .into_iter()
            .take(1),
    );
    lines
}

fn render_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) -> Option<Rect> {
    Some(match app.modal.as_mut()? {
        Modal::Help => {
            let area = popup(area, 52, 14);
            let inner = render_popup(frame, area, "Keyboard shortcuts");
            let lines = [
                "Enter       Send or steer",
                "Alt+Enter   Send on the next turn",
                "Esc         Close or interrupt",
                "↑ / ↓       Edit draft; select queued input",
                "Delete      Withdraw selected queued input",
                "F2          Focus a pending request",
                "Ctrl+P/F1   Show this help",
                "/ (empty draft) Search actions and Skills",
                "Esc         Close help",
            ];
            frame.render_widget(
                Paragraph::new(lines.join("\n")).style(Style::new().fg(MUTED)),
                inner,
            );
            inner
        }
        Modal::Picker(picker) => {
            let height = match picker.kind {
                PickerKind::Models | PickerKind::Thinking | PickerKind::Agents => {
                    let rows = picker.choice_count().clamp(4, 8) as u16;
                    9 + rows * 2
                }
                _ => PICKER_HEIGHT,
            };
            let area = popup(area, PICKER_WIDTH, height);
            let inner = render_popup(frame, area, picker.kind.title());
            let (width, page, header) = render_picker(frame, inner, picker);
            app.modal_editor_width = width;
            app.modal_page = page;
            app.modal_info_rows = header;
            inner
        }
        Modal::Prompt(prompt) => {
            let height = match prompt.kind {
                PromptKind::Agent | PromptKind::Model { .. } => SHORT_PROMPT_HEIGHT,
                PromptKind::Question { .. }
                | PromptKind::QueuedInput { .. }
                | PromptKind::RewindMessage { .. } => PICKER_HEIGHT,
            };
            let area = popup(area, PICKER_WIDTH, height);
            let inner = render_popup(frame, area, &prompt.title);
            let (width, header) = render_prompt(frame, inner, prompt);
            app.modal_editor_width = width;
            app.modal_info_rows = header;
            inner
        }
        Modal::Output(viewer) => {
            render_output(frame, popup(area, OUTPUT_WIDTH, area.height), viewer)
        }
        Modal::StatusLineEditor(editor) => {
            let available = Rect {
                height: area.height.saturating_sub(1),
                ..area
            };
            let area = popup(available, PICKER_WIDTH, STATUS_EDITOR_HEIGHT);
            let inner = render_popup(frame, area, "Status line settings");
            editor.render(frame, inner);
            inner
        }
    })
}

fn render_picker(frame: &mut Frame<'_>, inner: Rect, picker: &mut Picker) -> (usize, usize, usize) {
    if matches!(picker.kind, PickerKind::ModelDefault)
        && !picker.loading
        && picker.problem.is_none()
    {
        return render_model_default_confirmation(frame, inner, picker);
    }
    let width = usize::from(inner.width);
    let search_rows = u16::from(picker.kind.searchable() && inner.height >= 3);
    let search_gap = u16::from(search_rows > 0 && inner.height >= 10);
    let header = if picker.header.is_empty() {
        Vec::new()
    } else {
        wrap(
            render_literal(&picker.header, Style::default().fg(MUTED)),
            width,
        )
    };
    let header_count = usize::from(inner.height.saturating_sub(search_rows + 3)) / 2;
    let header_rows = header.len().min(header_count);
    picker.header_top = picker
        .header_top
        .min(header.len().saturating_sub(header_rows));
    let [information, search, _gap, list, footer] = Layout::vertical([
        Constraint::Length(header_rows as u16),
        Constraint::Length(search_rows),
        Constraint::Length(search_gap),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(
            header
                .into_iter()
                .skip(picker.header_top)
                .take(header_rows)
                .collect::<Vec<_>>(),
        ),
        information,
    );
    let marker_width = u16::from(search_rows > 0 && search.width >= 8) * 2;
    let [marker, field] =
        Layout::horizontal([Constraint::Length(marker_width), Constraint::Min(0)]).areas(search);
    if search_rows > 0 {
        frame.render_widget(Paragraph::new(Span::styled("/ ", emphasis(ACCENT))), marker);
        render_editor(frame, field, &mut picker.query, true, "Search…");
    }
    let cell_height = if list.height >= 6 { 2 } else { 1 };
    let count = usize::from(list.height / cell_height).max(1);
    let options = picker.visible_choices();
    let first = picker
        .selected
        .saturating_sub(count - 1)
        .min(options.len().saturating_sub(count));
    let last = (first + count).min(options.len());
    let mut lines = Vec::new();
    for (index, choice) in options.iter().enumerate().skip(first).take(count) {
        let active = index == picker.selected;
        let style = if active { focused_row() } else { panel() };
        let prefix = if active { "› " } else { "  " };
        let label = single_line(&choice.label);
        let detail = single_line(&choice.detail);
        let label = if cell_height == 1 && !detail.is_empty() {
            format!("{label} · {detail}")
        } else {
            label
        };
        lines.push(picker_line(
            &format!("{prefix}{label}"),
            width,
            active,
            style,
        ));
        if cell_height == 2 {
            lines.push(picker_line(
                &format!("  {detail}"),
                width,
                active,
                if active { style } else { style.fg(MUTED) },
            ));
        }
    }
    if options.is_empty() {
        lines.push(Line::styled(
            if picker.loading {
                "Loading…"
            } else {
                "No matching entries"
            },
            Style::default().fg(MUTED),
        ));
    }
    frame.render_widget(Paragraph::new(lines), list);
    picker.set_rows(list, first..last, cell_height);
    let footer_text = if let Some(error) = &picker.problem {
        Line::styled(
            shorten(&single_line(error), width),
            Style::default().fg(ERROR),
        )
    } else if width < 32 {
        Line::styled(
            shorten("↑↓ Select · Enter · Esc", width),
            Style::new().fg(MUTED),
        )
    } else {
        let select = format!("{}{} Select", Shortcut::UP.label, Shortcut::DOWN.label);
        let confirm = format!(
            "{} Confirm · {} Close",
            Shortcut::ENTER.label,
            Shortcut::ESCAPE.label
        );
        let help = if picker.kind.completes_text() {
            format!("{select} · {} Complete · {confirm}", Shortcut::TAB.label)
        } else if matches!(picker.kind, PickerKind::Server) {
            format!(
                "{}/{} Details · {select} · {confirm}",
                Shortcut::PAGE_UP.label,
                Shortcut::PAGE_DOWN.label,
            )
        } else {
            format!("{select} · {confirm}")
        };
        Line::styled(shorten(&help, width), Style::default().fg(MUTED))
    };
    frame.render_widget(Paragraph::new(footer_text), footer);
    (usize::from(field.width).max(1), count, header_rows)
}

fn render_model_default_confirmation(
    frame: &mut Frame<'_>,
    inner: Rect,
    picker: &mut Picker,
) -> (usize, usize, usize) {
    let width = usize::from(inner.width);
    let details = wrap(
        render_literal(&picker.header, Style::default().fg(MUTED)),
        width,
    );
    let scope = wrap(
        render_literal(SAVE_SCOPE, Style::default().fg(MUTED)),
        width,
    );
    let available_rows = usize::from(inner.height);
    let can_confirm = width >= 32 && available_rows >= scope.len().saturating_add(6);
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
    if can_confirm {
        let had_only_cancel = picker.choice_count() == 1;
        picker.set_local(vec![save, cancel]);
        if had_only_cancel {
            picker.selected = 1;
        }
    } else {
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

fn picker_line(text: &str, width: usize, selected: bool, style: Style) -> Line<'static> {
    let mut text = shorten(text, width);
    if selected {
        text.push_str(&" ".repeat(width - text.width()));
    }
    Line::styled(text, style)
}

fn render_prompt(frame: &mut Frame<'_>, inner: Rect, prompt: &mut Prompt) -> (usize, usize) {
    let width = usize::from(inner.width);
    let rewind_confirmation = matches!(prompt.kind, PromptKind::RewindMessage { .. });
    let instructions = wrap(
        render_literal(&prompt.instructions, Style::default().fg(MUTED)),
        width,
    );
    let header_rows = if rewind_confirmation {
        instructions
            .len()
            .min(usize::from(inner.height.saturating_sub(1)))
    } else {
        instructions
            .len()
            .min(usize::from(inner.height.saturating_sub(3)) / 2)
    };
    prompt.instructions_top = prompt
        .instructions_top
        .min(instructions.len().saturating_sub(header_rows));
    let [info, editor, help] = if rewind_confirmation {
        Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(0),
            Constraint::Length(1),
        ])
        .areas(inner)
    } else {
        Layout::vertical([
            Constraint::Length(header_rows as u16),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner)
    };
    frame.render_widget(
        Paragraph::new(
            instructions
                .into_iter()
                .skip(prompt.instructions_top)
                .take(header_rows)
                .collect::<Vec<_>>(),
        ),
        info,
    );
    if !rewind_confirmation {
        render_editor(frame, editor, &mut prompt.editor, true, "Type here…");
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            if rewind_confirmation {
                format!(
                    "{} Commit · Space Toggle file restore · {} Cancel",
                    Shortcut::ENTER.label,
                    Shortcut::ESCAPE.label,
                )
            } else {
                format!(
                    "{} Confirm · {} Clear · {}/{} Details · {} Close",
                    Shortcut::ENTER.label,
                    Shortcut::CLEAR.label,
                    Shortcut::PAGE_UP.label,
                    Shortcut::PAGE_DOWN.label,
                    Shortcut::ESCAPE.label,
                )
            },
            Style::default().fg(MUTED),
        )),
        help,
    );
    (usize::from(editor.width).max(1), header_rows)
}

fn render_output(frame: &mut Frame<'_>, area: Rect, viewer: &mut OutputViewer) -> Rect {
    let title = if viewer.tool.name.is_empty() {
        "Tool details"
    } else {
        &viewer.tool.name
    };
    let inner = render_popup(frame, area, title);
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let lines = viewer.render_lines(usize::from(body.width), usize::from(body.height));
    frame.render_widget(Paragraph::new(lines), body);
    frame.render_widget(Paragraph::new(viewer.footer()), footer);
    body
}

fn render_editor(
    frame: &mut Frame<'_>,
    area: Rect,
    editor: &mut Editor,
    focused: bool,
    placeholder: &str,
) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    let view = editor.project(width, height);
    let placeholder = editor.text().is_empty().then_some(placeholder);
    render_editor_view(frame, area, view, focused, placeholder);
}

fn render_editor_view(
    frame: &mut Frame<'_>,
    area: Rect,
    view: EditorView,
    focused: bool,
    placeholder: Option<&str>,
) {
    if let Some(placeholder) = placeholder {
        let label = shorten(placeholder, usize::from(area.width));
        frame.render_widget(
            Paragraph::new(Line::styled(label, Style::default().fg(MUTED))),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(view.lines.into_iter().map(Line::raw).collect::<Vec<_>>()),
            area,
        );
    }
    if focused {
        frame.set_cursor_position((area.x + view.cursor.0 as u16, area.y + view.cursor.1 as u16));
    }
}

fn render_popup(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    let screen = frame.area();
    frame
        .buffer_mut()
        .set_style(screen, crate::theme::backdrop());
    frame.render_widget(Clear, area);
    let block = Block::new().style(panel()).padding(panel_padding(area));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 {
        return inner;
    }
    let gap = u16::from(inner.height >= 9);
    let [title_area, _gap, content] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(gap),
        Constraint::Min(0),
    ])
    .areas(inner);
    let title = shorten(&single_line(title), usize::from(title_area.width));
    frame.render_widget(
        Paragraph::new(Span::styled(title, emphasis(ACCENT))),
        title_area,
    );
    content
}

fn popup(area: Rect, limit_width: u16, limit_height: u16) -> Rect {
    let horizontal = u16::from(area.width > 24) * 2;
    let vertical = u16::from(area.height > 10) * 2;
    let width = limit_width.min(area.width.saturating_sub(horizontal));
    let height = limit_height.min(area.height.saturating_sub(vertical));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests;
