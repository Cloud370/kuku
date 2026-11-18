use kuku_client::AgentStatus;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Modal, ServiceStatus};
use crate::editor::Editor;
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap, ACCENT, MUTED};
use crate::menu::{Picker, PickerKind, Prompt};
use crate::output::OutputViewer;
use crate::statusline::StatusLine;
use crate::text::{shorten, single_line};

const COMPOSER_HEIGHT: u16 = 6;
const NARROW_COMPOSER_HEIGHT: u16 = 4;
const MAX_BANNER_ROWS: u16 = 5;
const MIN_TRANSCRIPT_ROWS: u16 = 2;
const PICKER_WIDTH: u16 = 88;
const PICKER_HEIGHT: u16 = 25;
const OUTPUT_WIDTH: u16 = 112;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut App, statusline: &StatusLine) {
    let outer = frame.area();
    if outer.is_empty() {
        app.text_selection.clear();
        return;
    }
    let area = outer.inner(Margin::new(u16::from(outer.width >= 24), 0));
    let width = usize::from(area.width);
    let statusline = statusline.render_line(app, width);
    let status_height = u16::from(statusline.is_some());
    let editor_height = match area.height {
        0..=3 => area.height.saturating_sub(status_height),
        4..=11 => NARROW_COMPOSER_HEIGHT.min(area.height - status_height - 1),
        _ => COMPOSER_HEIGHT,
    };
    let banner_rows = area
        .height
        .saturating_sub(editor_height + status_height + MIN_TRANSCRIPT_ROWS)
        .min(MAX_BANNER_ROWS);
    let banner = banner(app, width, usize::from(banner_rows));
    let [transcript, banner_area, editor, status] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(banner.len() as u16),
        Constraint::Length(editor_height),
        Constraint::Length(status_height),
    ])
    .areas(area);
    let row_insets = render_transcript(frame, transcript, app, banner_area.height == 0);
    frame.render_widget(Paragraph::new(banner), banner_area);
    let composer_title = format!("You · draft to {}", app.active.agent);
    let composer_title = if composer_title.width() + 4 <= usize::from(editor.width) {
        composer_title
    } else {
        format!("Draft · {}", app.active.agent)
    };
    let composer_focused = app.modal.is_none();
    let focus_style = if composer_focused { Color::Cyan } else { MUTED };
    let composer = panel(&composer_title, focus_style, editor);
    let inner = composer.inner(editor);
    frame.render_widget(composer, editor);
    app.editor_width = usize::from(inner.width).max(1);
    render_editor(
        frame,
        inner,
        &mut app.pane_mut().draft,
        composer_focused,
        &format!("Type a prompt · {} Actions", Shortcut::ACTIONS.label),
    );
    if let Some(line) = statusline {
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
    let title = if app.pane().title.is_empty() {
        if app.pane().exists {
            "Session"
        } else {
            "New session"
        }
    } else {
        &app.pane().title
    };
    let title = shorten(
        &format!("kuku · {}", single_line(title)),
        usize::from(area.width),
    );
    let [header, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let (width, height) = (usize::from(body.width), usize::from(body.height));
    app.transcript_area = (width.max(1), height.max(1));
    let mut view = app.pane_mut().transcript.project(width, height);
    if view.lines.is_empty() && height > 0 {
        view.lines = welcome(app, width, height, show_service_problem);
    }
    if !view.following {
        let reading = format!("Reading · {} latest", Shortcut::LATEST.label);
        let gap = 3;
        let width = usize::from(header.width);
        if width > reading.width() + gap {
            let title = shorten(&title, width - reading.width() - gap);
            let padding = " ".repeat(width - title.width() - reading.width());
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(title, emphasis(ACCENT)),
                    Span::raw(padding),
                    Span::styled(reading, Style::default().fg(MUTED)),
                ])),
                header,
            );
        } else {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!("Reading · {}", Shortcut::LATEST.label),
                    emphasis(ACCENT),
                )),
                header,
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(Line::styled(title, emphasis(ACCENT))),
            header,
        );
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
        (error, Color::Yellow)
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
            problem.lines().take(2).map(|line| {
                Line::styled(shorten(&single_line(line), width), emphasis(Color::Yellow))
            }),
        );
    }
    if let Some(problem) = pane.model_problem.as_ref() {
        lines.push(Line::styled(
            shorten(
                &format!("Server configuration · /model · {}", single_line(problem)),
                width,
            ),
            emphasis(Color::Yellow),
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
                Style::default().fg(Color::Red),
            ));
        }
    }
    if let Some(notice) = &app.notice {
        let color = if notice.error { Color::Red } else { ACCENT };
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
    let style = emphasis(Color::Yellow);
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
            let area = popup(area, PICKER_WIDTH, PICKER_HEIGHT);
            let inner = render_popup(frame, area, picker.kind.title());
            let (width, page, header) = render_picker(frame, inner, picker);
            app.modal_editor_width = width;
            app.modal_page = page;
            app.modal_info_rows = header;
            inner
        }
        Modal::Prompt(prompt) => {
            let area = popup(area, PICKER_WIDTH, PICKER_HEIGHT);
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
    let header = if picker.header.is_empty() {
        Vec::new()
    } else {
        wrap(
            render_literal(&picker.header, Style::default().fg(Color::Gray)),
            width,
        )
    };
    let header_count = usize::from(inner.height.saturating_sub(search_rows + 3)) / 2;
    let header_rows = header.len().min(header_count);
    picker.header_top = picker
        .header_top
        .min(header.len().saturating_sub(header_rows));
    let [information, search, list, footer] = Layout::vertical([
        Constraint::Length(header_rows as u16),
        Constraint::Length(search_rows),
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
    if search_rows > 0 {
        render_editor(frame, search, &mut picker.query, true, "Search…");
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
        let color = if active { ACCENT } else { Color::Reset };
        let prefix = if active { "› " } else { "  " };
        let label = single_line(&choice.label);
        let detail = single_line(&choice.detail);
        let label = if cell_height == 1 && !detail.is_empty() {
            format!("{label} · {detail}")
        } else {
            label
        };
        lines.push(Line::styled(
            shorten(&format!("{prefix}{label}"), width),
            emphasis(color),
        ));
        if cell_height == 2 {
            lines.push(Line::styled(
                shorten(&format!("  {detail}"), width),
                Style::default().fg(MUTED),
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
            Style::default().fg(Color::Red),
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
    (width.max(1), count, header_rows)
}

fn render_prompt(frame: &mut Frame<'_>, inner: Rect, prompt: &mut Prompt) -> (usize, usize) {
    let width = usize::from(inner.width);
    let instructions = wrap(
        render_literal(&prompt.instructions, Style::default().fg(Color::Yellow)),
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
    if editor.text().is_empty() {
        let label = shorten(placeholder, width);
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

fn panel(title: &str, color: Color, area: Rect) -> Block<'static> {
    let title = shorten(
        &single_line(title),
        usize::from(area.width).saturating_sub(4),
    );
    if area.width < 5 || area.height < 3 {
        return Block::new();
    }
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Line::styled(format!(" {title} "), emphasis(color)))
}

fn render_popup(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    frame.render_widget(Clear, area);
    let block = panel(title, ACCENT, area);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
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

fn emphasis(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use kuku_client::{
        Action, Error, HistoryEntry, HistoryKind, HistoryPage, Observation, Snapshot, Timing,
    };
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use ratatui::{Frame, Terminal};
    use serde_json::json;

    use super::render as render_with_statusline;
    use crate::app::{Address, App, Modal};
    use crate::jobs::{Job, Update};
    use crate::output::OutputViewer;
    use crate::statusline::StatusLine;
    use crate::transcript::ToolView;

    fn render(frame: &mut Frame<'_>, app: &mut App) {
        render_with_statusline(frame, app, &StatusLine::default());
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .chunks(usize::from(width))
            .map(|row| row.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> App {
        let workspace = PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        app.effects.clear();
        app
    }

    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        event(app, Event::Key(KeyEvent::new(code, modifiers)));
    }

    fn event(app: &mut App, event: Event) {
        app.event(event, &Buffer::empty(Rect::ZERO));
    }

    fn mouse(kind: MouseEventKind, point: (u16, u16), modifiers: KeyModifiers) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: point.0,
            row: point.1,
            modifiers,
        })
    }

    fn cell(buffer: &Buffer, symbol: &str) -> (u16, u16) {
        let index = buffer
            .content
            .iter()
            .position(|cell| cell.symbol() == symbol)
            .unwrap_or_else(|| panic!("missing screen symbol {symbol:?}"));
        buffer.pos_of(index)
    }

    fn command(app: &mut App, name: &str) {
        key(app, KeyCode::F(1), KeyModifiers::NONE);
        for character in name.chars() {
            key(app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        key(app, KeyCode::Enter, KeyModifiers::NONE);
    }

    fn entry(id: u64, agent: &str, kind: HistoryKind, text: &str) -> HistoryEntry {
        HistoryEntry {
            id,
            agent: agent.into(),
            turn: Some(1),
            kind,
            text: Some(text.into()),
            details: Default::default(),
        }
    }

    fn snapshot(
        app: &App,
        status: &str,
        question: serde_json::Value,
        permission: serde_json::Value,
    ) -> Box<Snapshot> {
        Box::new(serde_json::from_value(json!({
            "session_id": app.active.session,
            "workspace": app.workspace,
            "cursor": 52,
            "pending": [],
            "agents": [],
            "selection": null,
            "agent": {
                "address": app.active.agent, "config": {}, "status": status, "pending_inputs": 0,
                "execution_id": 49, "question": question, "permission": permission,
                "usage": {"model_requests":0,"unreported_requests":0},
            }
        })).unwrap())
    }

    #[test]
    fn blank_configuration_screen_keeps_pasted_draft_on_wide_and_narrow_terminals() {
        let mut app = App::new(PathBuf::from("/workspace"), None);
        app.update(Update::ConnectFailed(Error::Options(
            "Run kuku init to configure a model".into(),
        )));
        let first = screen(&mut app, 104, 24);
        assert!(first.contains("New session"));
        assert!(first.contains("kuku init"));
        event(
            &mut app,
            Event::Paste("中文\r\nfirst\rlast e\u{301}".into()),
        );
        key(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!("中文\nfirst\nlast e\u{301}\nx", app.pane().draft.text());
        let view = screen(&mut app, 29, 13);
        assert!(view.contains("You · draft to main"));
        assert!(view.contains("last e\u{301}"));
        assert!(view.contains("Offline"));
    }

    #[test]
    fn switching_agent_keeps_drafts_and_read_position_and_ignores_old_observer() {
        let mut app = app();
        let main = app.active.clone();
        app.select(main.clone(), true, "Example".into());
        let main_selection = app.selection();
        let page = HistoryPage {
            entries: (1..46)
                .map(|id| {
                    entry(
                        id,
                        "main",
                        HistoryKind::Assistant,
                        &format!("main-reply-{id}"),
                    )
                })
                .collect(),
            next_before: Some(1),
        };
        app.update(Update::Loaded(
            main_selection.clone(),
            snapshot(&app, "running", json!(null), json!(null)),
            page,
        ));
        event(&mut app, Event::Paste("main-draft".into()));
        screen(&mut app, 79, 22);
        key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        let before = screen(&mut app, 79, 22);
        let review = Address::new(main.session.clone(), "reviewer".into());
        app.select(review.clone(), true, "Example".into());
        event(&mut app, Event::Paste("review-notes".into()));
        app.update(Update::Observation(
            main_selection,
            Observation::Live {
                agent: "main".into(),
                execution_id: 49,
                cursor: 52,
                turn: Some(1),
                event: json!({"type":"text","content":"obsolete main stream"}),
            },
        ));
        let other = screen(&mut app, 79, 22);
        assert!(other.contains("You · draft to reviewer"));
        assert!(other.contains("review-notes"));
        assert!(!other.contains("obsolete main stream"));
        app.select(main, true, "Example".into());
        let after = screen(&mut app, 79, 22);
        assert_eq!(
            before.lines().take(12).collect::<Vec<_>>(),
            after.lines().take(12).collect::<Vec<_>>()
        );
        assert!(after.contains("main-draft"));
        app.select(review, true, "Example".into());
        key(&mut app, KeyCode::Enter, KeyModifiers::ALT);
        let command = app
            .effects
            .iter()
            .find_map(|effect| {
                if let Job::Save(command) = effect {
                    Some(command)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!("reviewer", command.target.agent);
        assert!(
            matches!(&command.action, Action::Send { text, timing: Timing::NextTurn, .. } if text == "review-notes")
        );
    }

    #[test]
    fn waiting_request_is_explicit_and_escape_preserves_message_draft() {
        let mut app = app();
        app.select(app.active.clone(), true, "Waiting conversation".into());
        let target = app.selection();
        let waiting = snapshot(
            &app,
            "waiting",
            json!({
                "id":"q-1", "agent":"main", "turn":1,"tool_call_id":"tool-1","text":"Which test target?"
            }),
            json!(null),
        );
        app.update(Update::Loaded(
            target,
            waiting,
            HistoryPage {
                entries: vec![],
                next_before: None,
            },
        ));
        event(&mut app, Event::Paste("my independent draft".into()));
        let view = screen(&mut app, 82, 20);
        assert!(view.contains("Waiting · main · F2 Reply"));
        assert!(view.contains("Which test target?"));
        key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!("my independent draft", app.pane().draft.text());
        assert!(!app
            .effects
            .iter()
            .any(|effect| matches!(effect, Job::Save(_))));
        key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
        event(&mut app, Event::Paste("workspace tests".into()));
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let action = app
            .effects
            .iter()
            .find_map(|effect| {
                if let Job::Save(command) = effect {
                    Some(&command.action)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            matches!(action, Action::ReplyQuestion { request_id, text } if request_id == "q-1" && text == "workspace tests")
        );
        assert_eq!("my independent draft", app.pane().draft.text());
    }

    #[test]
    fn generic_tool_details_render_control_bytes_visibly_and_copy_original_reply() {
        let mut app = app();
        app.select(app.active.clone(), true, "Tool test".into());
        let reply = "## Result\n\n```rust\nlet number = 4;\n```";
        let tool_call = serde_json::from_value(json!({
            "id":3,"agent":"main","turn":1,"type":"tool_call","tool":"example_custom","tool_call_id":"t-1","args":{}
        })).unwrap();
        let tool_result = serde_json::from_value(json!({
            "id":4,"agent":"main","turn":1,"type":"tool_result","tool_call_id":"t-1","status":"ok","summary":"finished",
            "model_content":"before\u{001b}[2Jafter","output_id":"saved-output","structured":{"exit_code":0}
        })).unwrap();
        let page = HistoryPage {
            entries: vec![
                entry(2, "main", HistoryKind::Assistant, reply),
                tool_call,
                tool_result,
            ],
            next_before: None,
        };
        app.update(Update::Loaded(
            app.selection(),
            snapshot(&app, "idle", json!(null), json!(null)),
            page,
        ));
        let view = screen(&mut app, 72, 25);
        assert!(view.contains("Tool example_custom · ok · exit 0"));
        assert!(view.contains("before␛[2Jafter"));
        assert!(view.contains("let number = 4;"));
        assert!(!view.contains('\u{001b}'));
        key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        let view = screen(&mut app, 72, 25);
        assert!(view.contains("Ctrl+O show"));
        assert!(!view.contains("before␛[2Jafter"));
        key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert!(screen(&mut app, 72, 25).contains("before␛[2Jafter"));
        command(&mut app, "copy");
        assert!(app
            .effects
            .iter()
            .any(|effect| matches!(effect, Job::Copy(text) if text == reply)));
    }

    #[test]
    fn mouse_drag_highlights_and_copies_visible_unicode_in_both_directions() {
        let mut app = app();
        app.pane_mut().transcript.apply(
            entry(
                1,
                "main",
                HistoryKind::Assistant,
                "before **中e\u{301}**\n\n👩‍💻! after",
            ),
            None,
        );
        let mut terminal = Terminal::new(TestBackend::new(72, 22)).unwrap();
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        let wide = cell(buffer, "中");
        let start = (wide.0 + 1, wide.1);
        let end = cell(buffer, "!");
        let emoji = cell(buffer, "👩‍💻");
        let left = MouseButton::Left;
        let modifiers = KeyModifiers::NONE;

        for (anchor, release) in [(start, end), (end, start)] {
            let buffer = terminal
                .draw(|frame| render(frame, &mut app))
                .unwrap()
                .buffer;
            app.event(mouse(MouseEventKind::Down(left), anchor, modifiers), buffer);
            app.event(mouse(MouseEventKind::Drag(left), emoji, modifiers), buffer);
            let buffer = terminal
                .draw(|frame| render(frame, &mut app))
                .unwrap()
                .buffer;
            assert!(buffer[emoji].modifier.contains(Modifier::REVERSED));
            if anchor == start {
                assert!(buffer[wide].modifier.contains(Modifier::REVERSED));
                assert!(buffer[start].modifier.contains(Modifier::REVERSED));
                assert!(!buffer[end].modifier.contains(Modifier::REVERSED));
            }
            assert!(app.effects.is_empty());
            app.event(mouse(MouseEventKind::Up(left), release, modifiers), buffer);
            assert!(matches!(
                std::mem::take(&mut app.effects).as_slice(),
                [Job::Copy(text)] if text == "中e\u{301}\n\n👩‍💻!"
            ));
            app.event(mouse(MouseEventKind::Up(left), release, modifiers), buffer);
            assert!(app.effects.is_empty());
        }
    }

    #[test]
    fn mouse_drag_copies_sanitized_text_from_tool_output_popup() {
        let mut app = app();
        app.modal = Some(Modal::Output(Box::new(OutputViewer::new(
            1,
            ToolView {
                name: "Tool".into(),
                summary: "Finished".into(),
                invocation: String::new(),
                details: "before\u{001b}[2J\nExit".into(),
                diff: String::new(),
                output_id: None,
            },
        ))));
        let mut terminal = Terminal::new(TestBackend::new(58, 18)).unwrap();
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        let start = cell(buffer, "␛");
        let end = cell(buffer, "E");
        let left = MouseButton::Left;
        let modifiers = KeyModifiers::NONE;
        app.event(mouse(MouseEventKind::Down(left), start, modifiers), buffer);
        app.event(mouse(MouseEventKind::Drag(left), end, modifiers), buffer);
        app.event(mouse(MouseEventKind::Up(left), end, modifiers), buffer);
        assert!(matches!(app.effects.as_slice(), [Job::Copy(text)] if text == "␛[2J\nE"));
    }

    #[test]
    fn click_shift_drag_and_replaced_text_leave_the_clipboard_alone() {
        let mut app = app();
        app.pane_mut().draft.insert("abc XYZ");
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        let left = MouseButton::Left;
        let start = cell(buffer, "X");
        let end = cell(buffer, "Z");
        let down = mouse(MouseEventKind::Down(left), start, KeyModifiers::NONE);
        let up = mouse(MouseEventKind::Up(left), end, KeyModifiers::NONE);
        app.event(down.clone(), buffer);
        app.event(
            mouse(MouseEventKind::Up(left), start, KeyModifiers::NONE),
            buffer,
        );
        for (kind, point) in [
            (MouseEventKind::Down(left), start),
            (MouseEventKind::Drag(left), end),
            (MouseEventKind::Up(left), end),
        ] {
            app.event(mouse(kind, point, KeyModifiers::SHIFT), buffer);
        }
        assert!(app.effects.is_empty());

        app.event(down, buffer);
        app.event(
            mouse(MouseEventKind::Drag(left), end, KeyModifiers::NONE),
            buffer,
        );
        app.pane_mut().draft.set_text("abc UVW".into());
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        app.event(up, buffer);
        assert!(app.effects.is_empty());
        assert!(!buffer[start].modifier.contains(Modifier::REVERSED));
    }
}
