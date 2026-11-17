use kuku_client::AgentStatus;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Modal, Pane};
use crate::editor::Editor;
use crate::jobs::agent_status;
use crate::markup::{render_literal, wrap, ACCENT, MUTED};
use crate::menu::{Picker, PickerKind, Prompt};
use crate::output::OutputViewer;
use crate::text::{shorten, single_line};

const COMPOSER_HEIGHT: u16 = 6;
const NARROW_COMPOSER_HEIGHT: u16 = 4;
const PICKER_WIDTH: u16 = 88;
const PICKER_HEIGHT: u16 = 25;
const OUTPUT_WIDTH: u16 = 112;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut App) {
    let outer = frame.area();
    if outer.is_empty() {
        return;
    }
    let area = outer.inner(Margin::new(u16::from(outer.width >= 24), 0));
    let banner = banner(app, usize::from(area.width), usize::from(area.height));
    let editor_height = match area.height {
        0..=3 => area.height.saturating_sub(1),
        4..=11 => NARROW_COMPOSER_HEIGHT.min(area.height - 2),
        _ => COMPOSER_HEIGHT,
    };
    let [transcript, banner_area, editor, status] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(banner.len() as u16),
        Constraint::Length(editor_height),
        Constraint::Length(1),
    ])
    .areas(area);
    render_transcript(frame, transcript, app);
    frame.render_widget(Paragraph::new(banner), banner_area);
    let composer_title =
        if app.active.agent.width() + "Message ".width() + 4 <= usize::from(editor.width) {
            format!("Message {}", app.active.agent)
        } else {
            format!("To {}", app.active.agent)
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
        "Type a prompt · / Actions",
    );
    render_status(frame, status, app);
    render_modal(frame, outer, app);
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if area.is_empty() {
        app.transcript_area = (usize::from(area.width).max(1), 1);
        return;
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
        view.lines = welcome(app, width, height);
    }
    if !view.following {
        let reading = "Reading · Ctrl+End latest";
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
                Paragraph::new(Line::styled("Reading · Ctrl+End", emphasis(ACCENT))),
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
}

fn welcome(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    let pane = app.pane();
    let (text, color) = if let Some(error) = &app.connection_problem {
        (error.as_str(), Color::Yellow)
    } else if app.connecting {
        ("Connecting to the Session service…", MUTED)
    } else if pane.opening {
        ("Opening the conversation…", MUTED)
    } else if let Some(error) = pane.observation_problem() {
        (error, Color::Yellow)
    } else if pane.exists {
        ("Send a message to this Agent.", MUTED)
    } else {
        ("Enter sends the first prompt. Shift+Enter or Ctrl+J adds a line.\n/resume opens a saved conversation.", MUTED)
    };
    wrap(render_literal(text, Style::default().fg(color)), width)
        .into_iter()
        .take(height)
        .collect()
}

fn banner(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    if height < COMPOSER_HEIGHT as usize + 3 {
        return Vec::new();
    }
    let pane = app.pane();
    let mut lines = Vec::new();
    if let Some(wait) = waiting(pane, &app.active.agent) {
        lines.extend(
            wrap(render_literal(&wait, emphasis(Color::Yellow)), width)
                .into_iter()
                .take(2),
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
    if pane.exists {
        if let Some(problem) = app
            .connection_problem
            .as_deref()
            .or_else(|| pane.observation_problem())
        {
            lines.push(Line::styled(
                shorten(&single_line(problem), width),
                emphasis(Color::Yellow),
            ));
        } else if let Some(snapshot) = pane
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.agent.last_error.is_some())
        {
            let error = snapshot.agent.last_error.as_deref().unwrap_or_default();
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
    lines.truncate(height.saturating_sub(10).clamp(1, 5));
    lines
}

fn waiting(pane: &Pane, agent: &str) -> Option<String> {
    let snapshot = pane.snapshot.as_ref()?;
    if let Some(question) = &snapshot.agent.question {
        Some(format!(
            "Waiting · {} · F2 Reply\n{}",
            single_line(agent),
            question.text
        ))
    } else {
        snapshot.agent.permission.as_ref().map(|permission| {
            format!(
                "Waiting · {} · F2 Review authorization\n{}",
                single_line(agent),
                permission.summary
            )
        })
    }
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let pane = app.pane();
    let queued = pane
        .snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.agent.pending_inputs);
    let state = activity(app);
    let color = match state {
        "Offline" | "Failed" | "Interrupted" => Color::Red,
        "Waiting" | "Stopping" => Color::Yellow,
        "Running" | "Sending" => Color::Green,
        _ => ACCENT,
    };
    let state = if queued > 0 {
        format!("{state} · {queued} queued")
    } else {
        state.to_owned()
    };
    let server = match &app.endpoint {
        Some(endpoint) => format!(
            "{} {}",
            if app.remote { "Remote" } else { "Local" },
            single_line(endpoint)
        ),
        None => "Local server".to_owned(),
    };
    let workspace = single_line(&app.workspace.to_string_lossy());
    let route = pane
        .effective_model_label()
        .map(|route| single_line(&route))
        .unwrap_or_else(|| "Model pending".into());
    let mut rest = [server, workspace, route];
    let mut width = usize::from(area.width).saturating_sub(state.width());
    if width < 12 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                shorten(&state, usize::from(area.width)),
                emphasis(color),
            )),
            area,
        );
        return;
    }
    let separator = " · ";
    let mut spans = vec![Span::styled(state, emphasis(color))];
    width = width.saturating_sub(separator.width() * rest.len());
    let fixed_width = rest.iter().map(|part| part.width()).sum::<usize>();
    if fixed_width > width {
        let mut missing = rest.len();
        for part in &mut rest {
            *part = shorten_middle(part, width / missing);
            width = width.saturating_sub(part.width());
            missing -= 1;
        }
    }
    for part in rest {
        spans.push(Span::styled(separator, Style::default().fg(MUTED)));
        spans.push(Span::raw(part));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn activity(app: &App) -> &'static str {
    let pane = app.pane();
    if app.connecting {
        return "Connecting";
    }
    if app.restarting {
        return "Restarting";
    }
    if app.connection_problem.is_some() || pane.observation_problem().is_some() {
        return "Offline";
    }
    if pane.opening {
        return "Opening";
    }
    if pane.stopping
        || pane
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.cleaning_up)
    {
        return "Stopping";
    }
    if pane.sending {
        return "Sending";
    }
    if let Some(snapshot) = &pane.snapshot {
        if snapshot.agent.last_error.is_some() && snapshot.agent.status == AgentStatus::Idle {
            return "Failed";
        }
        return agent_status(snapshot.agent.status);
    }
    if pane.model_problem.is_some() {
        "Configuration"
    } else {
        "Ready"
    }
}

fn render_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let Some(modal) = &mut app.modal else { return };
    match modal {
        Modal::Picker(picker) => {
            let area = popup(area, PICKER_WIDTH, PICKER_HEIGHT);
            let (width, page, header) = render_picker(frame, area, picker);
            app.modal_editor_width = width;
            app.modal_page = page;
            app.modal_info_rows = header;
        }
        Modal::Prompt(prompt) => {
            let area = popup(area, PICKER_WIDTH, PICKER_HEIGHT);
            let (width, header) = render_prompt(frame, area, prompt);
            app.modal_editor_width = width;
            app.modal_info_rows = header;
        }
        Modal::Output(viewer) => {
            render_output(frame, popup(area, OUTPUT_WIDTH, area.height), viewer)
        }
    }
}

fn render_picker(frame: &mut Frame<'_>, area: Rect, picker: &mut Picker) -> (usize, usize, usize) {
    let inner = render_popup(frame, area, picker.kind.title());
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
        let help = match picker.kind {
            PickerKind::Server | PickerKind::Permission => {
                "PgUp/PgDn Details · ↑↓ Select · Enter Confirm · Esc Close"
            }
            _ => "↑↓ Select · Enter Open · Esc Close",
        };
        Line::styled(shorten(help, width), Style::default().fg(MUTED))
    };
    frame.render_widget(Paragraph::new(footer_text), footer);
    (width.max(1), count, header_rows)
}

fn render_prompt(frame: &mut Frame<'_>, area: Rect, prompt: &mut Prompt) -> (usize, usize) {
    let inner = render_popup(frame, area, &prompt.title);
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
            "Enter Confirm · PgUp/PgDn Details · Esc Close",
            Style::default().fg(MUTED),
        )),
        help,
    );
    (usize::from(editor.width).max(1), header_rows)
}

fn render_output(frame: &mut Frame<'_>, area: Rect, viewer: &mut OutputViewer) {
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

fn shorten_middle(text: &str, width: usize) -> String {
    if text.width() <= width || width < 8 {
        return shorten(text, width);
    }
    let tail_width = width / 2;
    let mut cells = 0;
    let mut tail = text.len();
    for (offset, character) in text.char_indices().rev() {
        let columns = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if cells + columns > tail_width {
            break;
        }
        tail = offset;
        cells += columns;
    }
    format!(
        "{}{}",
        shorten(&text[..tail], width.saturating_sub(cells)),
        &text[tail..]
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use kuku_client::{
        Action, Error, HistoryEntry, HistoryKind, HistoryPage, Observation, Snapshot, Timing,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;

    use super::render;
    use crate::app::{Address, App};
    use crate::jobs::{Job, Update};

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
        app.event(Event::Key(KeyEvent::new(code, modifiers)));
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
        app.event(Event::Paste("中文\r\nfirst\rlast e\u{301}".into()));
        key(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!("中文\nfirst\nlast e\u{301}\nx", app.pane().draft.text());
        let view = screen(&mut app, 29, 13);
        assert!(view.contains("Message main"));
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
        app.event(Event::Paste("main-draft".into()));
        screen(&mut app, 79, 22);
        key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        let before = screen(&mut app, 79, 22);
        let review = Address::new(main.session.clone(), "reviewer".into());
        app.select(review.clone(), true, "Example".into());
        app.event(Event::Paste("review-notes".into()));
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
        assert!(other.contains("Message reviewer"));
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
        app.event(Event::Paste("my independent draft".into()));
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
        app.event(Event::Paste("workspace tests".into()));
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
        key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        let view = screen(&mut app, 72, 25);
        assert!(view.contains("example_custom · ok · exit 0"));
        assert!(view.contains("before␛[2Jafter"));
        assert!(view.contains("let number = 4;"));
        assert!(!view.contains('\u{001b}'));
        command(&mut app, "copy");
        assert!(app
            .effects
            .iter()
            .any(|effect| matches!(effect, Job::Copy(text) if text == reply)));
    }
}
