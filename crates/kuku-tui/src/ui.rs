use kuku_client::AgentStatus;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Modal, ServiceStatus};
use crate::editor::{Editor, EditorView};
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap};
use crate::menu::{Picker, PickerKind, Prompt, PromptKind};
use crate::output::OutputViewer;
use crate::statusline::StatusLine;
use crate::text::{shorten, single_line};
use crate::theme::{
    composer_padding, emphasis, focused_row, horizontal_gutter, panel, panel_padding, surface,
    ACCENT, ERROR, MAX_DRAFT_ROWS, MUTED, OUTPUT_WIDTH, PICKER_HEIGHT, PICKER_WIDTH, SHORT_PROMPT_HEIGHT, WARNING,
};

const MAX_BANNER_ROWS: u16 = 5;
const MIN_TRANSCRIPT_ROWS: u16 = 2;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut App, statusline: &StatusLine) {
    let outer = frame.area();
    if outer.is_empty() {
        app.text_selection.clear();
        return;
    }
    let area = outer.inner(Margin::new(horizontal_gutter(outer.width), 0));
    let width = usize::from(area.width);
    let footer = statusline.render_line(app, width);
    let status_height = u16::from(footer.is_some());
    let chrome_rows = if area.height > 3 {
        MIN_TRANSCRIPT_ROWS
    } else {
        0
    };
    let padding = composer_padding(area);
    let padding_rows = padding.top + padding.bottom;
    let composer_gap = u16::from(area.height >= 16);
    let editor_limit = area
        .height
        .saturating_sub(status_height + chrome_rows + padding_rows + composer_gap);
    let focused = app.modal.is_none();
    let mut composer = Block::new().style(surface());
    if area.width >= 4 {
        composer = composer
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(if focused { ACCENT } else { MUTED }))
            .padding(padding);
    }
    let content_width = usize::from(composer.inner(area).width);
    let heading_rows = u16::from(editor_limit >= 2);
    let draft_limit = usize::from(editor_limit - heading_rows).min(MAX_DRAFT_ROWS);
    let draft = (draft_limit > 0).then(|| app.pane_mut().draft.project(content_width, draft_limit));
    let editor_height =
        padding_rows + heading_rows + draft.as_ref().map_or(0, |view| view.lines.len() as u16);
    let banner_rows = area
        .height
        .saturating_sub(editor_height + composer_gap + status_height + MIN_TRANSCRIPT_ROWS)
        .min(MAX_BANNER_ROWS);
    let banner = banner(app, width, usize::from(banner_rows));
    let [transcript, banner_area, _gap, editor, status] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(banner.len() as u16),
        Constraint::Length(composer_gap),
        Constraint::Length(editor_height),
        Constraint::Length(status_height),
    ])
    .areas(area);
    let row_insets = render_transcript(frame, transcript, app, banner_area.height == 0);
    frame.render_widget(Paragraph::new(banner), banner_area);
    let [heading, inner] = Layout::vertical([Constraint::Length(heading_rows), Constraint::Min(0)])
        .areas(composer.inner(editor));
    frame.render_widget(composer, editor);
    app.editor_width = content_width.max(1);
    if !heading.is_empty() {
        let line = statusline.render_composer_heading(app, usize::from(heading.width));
        frame.render_widget(Paragraph::new(line), heading);
    }
    if let Some(draft) = draft {
        let placeholder = app
            .pane()
            .draft
            .text()
            .is_empty()
            .then(|| format!("Type a prompt · {} Actions", Shortcut::ACTIONS.label));
        render_editor_view(frame, inner, draft, focused, placeholder.as_deref());
    }
    if let Some(line) = footer {
        frame.render_widget(Paragraph::new(line), status);
    }
    let main_areas = [transcript, banner_area, inner, status];
    let modal_area = render_modal(frame, outer, app);
    app.text_selection.render(
        frame.buffer_mut(),
        modal_area
            .as_ref()
            .map_or(&main_areas[..], std::slice::from_ref),
        modal_area.is_none().then_some((transcript, row_insets)),
    );
}

fn render_transcript(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    show_service_problem: bool,
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
    if view.lines.is_empty() && height > 0 {
        view.lines = welcome(app, width, height, show_service_problem);
    }
    let session_label = if app.pane().exists {
        let id: String = single_line(&app.active.session).chars().take(8).collect();
        format!("Session {id}")
    } else {
        "New session".into()
    };
    let hint = if view.following {
        view.tool_hint.unwrap_or_default()
    } else {
        format!("Reading · {} latest", Shortcut::LATEST.label)
    };
    let hint_width = if hint.width() + 20 <= width {
        hint.width()
    } else {
        0
    };
    let available = width.saturating_sub(hint_width + usize::from(hint_width > 0));
    let label = shorten(&session_label, available);
    let mut spans = vec![Span::styled(label.clone(), emphasis(ACCENT))];
    let title = single_line(&app.pane().title);
    if !title.is_empty() && available > label.width() + 3 {
        let title = shorten(&title, available - label.width() - 3);
        spans.push(Span::styled(" · ", Style::default().fg(MUTED)));
        spans.push(Span::styled(title, emphasis(ratatui::style::Color::Reset)));
    }
    let mut line = Line::from(spans);
    if hint_width > 0 {
        line.push_span(" ".repeat(width - line.width() - hint_width));
        line.push_span(Span::styled(hint, Style::default().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(line), header);
    for (row, line) in view.lines.iter().enumerate() {
        if line.style.bg.is_some() {
            let area = Rect::new(body.x, body.y + row as u16, body.width, 1);
            frame.buffer_mut().set_style(area, line.style);
        }
    }
    frame.render_widget(Paragraph::new(view.lines), body);
    view.row_insets.insert(0, 0);
    view.row_insets
}

fn welcome(
    app: &App,
    width: usize,
    height: usize,
    show_service_problem: bool,
) -> Vec<Line<'static>> {
    let pane = app.pane();
    let help;
    let status = app.service_status();
    let (text, color) = if let Some(error) = app.service_problem() {
        if !show_service_problem {
            return Vec::new();
        }
        (error, WARNING)
    } else if pane.opening {
        ("Opening the conversation…", MUTED)
    } else if status == ServiceStatus::Connecting {
        ("Connecting to the Session service…", MUTED)
    } else if status == ServiceStatus::Reconnecting {
        ("Reconnecting to the Session service…", MUTED)
    } else if status == ServiceStatus::Restarting {
        ("Restarting the Session service…", MUTED)
    } else if pane.exists {
        ("Send a message to this Agent.", MUTED)
    } else {
        help = format!(
            "{} sends the first prompt. {} or {} adds a line.\n/resume opens a saved conversation.",
            Shortcut::ENTER.label,
            Shortcut::NEW_LINE.label,
            Shortcut::CONTROL_NEW_LINE.label,
        );
        (help.as_str(), MUTED)
    };
    wrap(render_literal(text, Style::default().fg(color)), width)
        .into_iter()
        .take(height)
        .collect()
}

fn banner(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    let pane = app.pane();
    let mut lines = waiting(app, width);
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
                format!("Execution · /model or /continue · {}", single_line(error))
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
                PromptKind::Question { .. } => PICKER_HEIGHT,
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
    })
}

fn render_picker(frame: &mut Frame<'_>, inner: Rect, picker: &mut Picker) -> (usize, usize, usize) {
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
    let count = (usize::from(list.height) / cell_height).max(1);
    let options = picker.visible_choices();
    let first = picker
        .selected
        .saturating_sub(count - 1)
        .min(options.len().saturating_sub(count));
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
        } else if matches!(picker.kind, PickerKind::Server | PickerKind::Permission) {
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

fn picker_line(text: &str, width: usize, selected: bool, style: Style) -> Line<'static> {
    let mut text = shorten(text, width);
    if selected {
        text.push_str(&" ".repeat(width - text.width()));
    }
    Line::styled(text, style)
}

fn render_prompt(frame: &mut Frame<'_>, inner: Rect, prompt: &mut Prompt) -> (usize, usize) {
    let width = usize::from(inner.width);
    let instructions = wrap(
        render_literal(&prompt.instructions, Style::default().fg(MUTED)),
        width,
    );
    let header_rows = instructions
        .len()
        .min(usize::from(inner.height.saturating_sub(3)) / 2);
    prompt.instructions_top = prompt
        .instructions_top
        .min(instructions.len().saturating_sub(header_rows));
    let [info, editor, help] = Layout::vertical([
        Constraint::Length(header_rows as u16),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
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
    render_editor(frame, editor, &mut prompt.editor, true, "Type here…");
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(
                "{} Confirm · {} Clear · {}/{} Details · {} Close",
                Shortcut::ENTER.label,
                Shortcut::CLEAR.label,
                Shortcut::PAGE_UP.label,
                Shortcut::PAGE_DOWN.label,
                Shortcut::ESCAPE.label,
            ),
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
