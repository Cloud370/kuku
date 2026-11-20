use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};

use crate::editor::Editor;
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::config::Settings;
use super::state::{SetupState, SetupStep};
use super::{CredentialSource, Finish, SetupMode};

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 20;

pub(super) fn run(
    settings: &Settings,
    state: &mut SetupState,
    mode: SetupMode,
) -> io::Result<Finish> {
    let _restore = TerminalRestore::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    loop {
        terminal.draw(|frame| render(frame, settings, state, mode))?;
        if let Some(finish) = state.finish {
            if finish == Finish::Cancel {
                return Ok(finish);
            }
            let size = terminal.size()?;
            if usable(size.width, size.height) {
                return Ok(finish);
            }
            state.finish = None;
            state.problem = Some("Resize to at least 60 columns by 20 rows before saving.".into());
        }
        if event::poll(Duration::from_millis(100))? {
            let event = event::read()?;
            let size = terminal.size()?;
            if !usable(size.width, size.height) {
                if handle_small_terminal_event(state, &event) {
                    return Ok(Finish::Cancel);
                }
                continue;
            }
            state.handle(event, settings, mode);
        }
    }
}

pub(super) fn usable(width: u16, height: u16) -> bool {
    width >= MIN_WIDTH && height >= MIN_HEIGHT
}

fn is_cancel(event: &Event) -> bool {
    match event {
        Event::Key(KeyEvent {
            code: KeyCode::Esc, ..
        }) => true,
        Event::Key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        }) => modifiers.contains(KeyModifiers::CONTROL),
        _ => false,
    }
}

pub(super) fn handle_small_terminal_event(state: &mut SetupState, event: &Event) -> bool {
    if is_cancel(event) {
        state.finish = Some(Finish::Cancel);
        true
    } else {
        false
    }
}

pub(super) fn render(
    frame: &mut Frame<'_>,
    settings: &Settings,
    state: &mut SetupState,
    mode: SetupMode,
) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" kuku · shared settings ")
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if !usable(area.width, area.height) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("Terminal too small.", Style::default().fg(WARNING)),
                Line::raw("Minimum: 60 columns by 20 rows."),
                Line::raw("Resize to continue."),
                Line::raw("Esc / Ctrl+C cancels without saving."),
            ]),
            inner,
        );
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Workspace: {}", shortened(&settings.workspace))),
            Line::from(format!("Config: {}", shortened(&settings.config_path))),
        ]),
        chunks[0],
    );
    render_step(frame, chunks[1], settings, state, mode);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "↑/↓ select · Tab next · Shift+Tab back · Enter confirm",
                Style::default().fg(MUTED),
            ),
            Line::styled(
                "Esc back/cancel · Ctrl+C cancel · Ctrl+A clear field",
                Style::default().fg(MUTED),
            ),
        ]),
        chunks[2],
    );
}

fn render_step(
    frame: &mut Frame<'_>,
    area: Rect,
    settings: &Settings,
    state: &mut SetupState,
    mode: SetupMode,
) {
    let mut row = 0;
    match state.step {
        SetupStep::Connection => {
            line(frame, area, &mut row, "Step 1/5 · Connection", ACCENT);
            line(
                frame,
                area,
                &mut row,
                "Existing connections and provider presets:",
                MUTED,
            );
            let available = area.height.saturating_sub(row + 3) as usize;
            let first = state.selected.saturating_sub(available / 2);
            let last = (first + available).min(state.choices.len());
            for index in first..last {
                let selected = index == state.selected;
                let label = settings.choice_label(&state.choices[index]);
                let marker = if selected { "› " } else { "  " };
                let style = if selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                render_line(
                    frame,
                    area,
                    &mut row,
                    Line::styled(format!("{marker}{label}"), style),
                );
            }
        }
        SetupStep::Model => {
            line(frame, area, &mut row, "Step 2/5 · Model ID", ACCENT);
            line(
                frame,
                area,
                &mut row,
                "This edits the current default tier; availability is not checked.",
                MUTED,
            );
            field(frame, area, &mut row, &mut state.model);
            line(
                frame,
                area,
                &mut row,
                &format!("Connection: {}", state.connection_label(settings)),
                MUTED,
            );
        }
        SetupStep::CredentialSource => {
            line(
                frame,
                area,
                &mut row,
                "Step 3/5 · Credential source",
                ACCENT,
            );
            let environment_status = if state.environment_set() {
                "detected"
            } else {
                "not detected"
            };
            for (source, label) in [
                (
                    CredentialSource::Environment,
                    format!(
                        "Environment · {environment_status} · {}",
                        state.environment.text().trim()
                    ),
                ),
                (
                    CredentialSource::StoredKey,
                    "Paste API key (save locally)".into(),
                ),
            ] {
                let selected = source == state.credential_source;
                let marker = if selected { "› " } else { "  " };
                let style = if selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                render_line(
                    frame,
                    area,
                    &mut row,
                    Line::styled(format!("{marker}{label}"), style),
                );
            }
            line(
                frame,
                area,
                &mut row,
                "Presence only; credential not authenticated.",
                MUTED,
            );
            if !settings.can_store_key() {
                line(frame, area, &mut row, settings.storage_problem(), WARNING);
            }
        }
        SetupStep::Credential => {
            line(frame, area, &mut row, "Step 4/5 · Credential", ACCENT);
            match state.credential_source {
                CredentialSource::Environment => {
                    line(frame, area, &mut row, "Enter a variable name only; key values are never displayed or stored here.", MUTED);
                    field(frame, area, &mut row, &mut state.environment);
                    let status = if state.environment_set() {
                        "Variable is set (credential not validated)."
                    } else {
                        "Variable is missing; it can be set after saving."
                    };
                    line(
                        frame,
                        area,
                        &mut row,
                        status,
                        if state.environment_set() {
                            SUCCESS
                        } else {
                            WARNING
                        },
                    );
                }
                CredentialSource::StoredKey => {
                    line(
                        frame,
                        area,
                        &mut row,
                        "Paste a key; every character is masked.",
                        MUTED,
                    );
                    masked_field(frame, area, &mut row, &state.stored_key);
                    line(
                        frame,
                        area,
                        &mut row,
                        "Saved locally as plaintext, not encrypted.",
                        WARNING,
                    );
                    line(
                        frame,
                        area,
                        &mut row,
                        &format!("Location: {}", shortened(&settings.credentials_dir)),
                        MUTED,
                    );
                    if let Some(hint) = state.saved_key_hint(settings) {
                        line(frame, area, &mut row, hint, SUCCESS);
                    }
                }
            }
        }
        SetupStep::Review => {
            line(frame, area, &mut row, "Step 5/5 · Review", ACCENT);
            line(
                frame,
                area,
                &mut row,
                &format!("Connection: {}", state.connection_label(settings)),
                MUTED,
            );
            line(
                frame,
                area,
                &mut row,
                &format!("Default tier: {}", settings.default_tier),
                MUTED,
            );
            line(
                frame,
                area,
                &mut row,
                &format!("Model: {}", state.model.text().trim()),
                MUTED,
            );
            match state.credential_source {
                CredentialSource::Environment => {
                    line(
                        frame,
                        area,
                        &mut row,
                        &format!("Environment: {}", state.environment.text().trim()),
                        MUTED,
                    );
                    let status = if state.environment_set() {
                        "Variable is set; credential not validated."
                    } else {
                        "Variable not detected; set it in the launch shell."
                    };
                    line(
                        frame,
                        area,
                        &mut row,
                        status,
                        if state.environment_set() {
                            SUCCESS
                        } else {
                            WARNING
                        },
                    );
                }
                CredentialSource::StoredKey => {
                    line(
                        frame,
                        area,
                        &mut row,
                        "Saved locally as plaintext, not encrypted.",
                        WARNING,
                    );
                    line(
                        frame,
                        area,
                        &mut row,
                        &format!("Location: {}", shortened(&settings.credentials_dir)),
                        MUTED,
                    );
                    if let Some(hint) = state.saved_key_hint(settings) {
                        line(frame, area, &mut row, hint, SUCCESS);
                    } else if !state.stored_key_is_entered() {
                        line(
                            frame,
                            area,
                            &mut row,
                            "Enter a key before saving this source.",
                            WARNING,
                        );
                    }
                }
            }
            line(
                frame,
                area,
                &mut row,
                "Setup makes no model request.",
                MUTED,
            );
            if mode == SetupMode::FirstRun && !state.credential_ready() {
                let guidance = if settings.can_store_key() {
                    "Choose Paste API key to repair this credential."
                } else {
                    "Storage unavailable; set the variable before launch."
                };
                line(
                    frame,
                    area,
                    &mut row,
                    guidance,
                    if settings.can_store_key() {
                        MUTED
                    } else {
                        WARNING
                    },
                );
            }
            for (index, action) in state.review_choices(mode).iter().enumerate() {
                let marker = if index == state.review_selection {
                    "› "
                } else {
                    "  "
                };
                let style = if index == state.review_selection {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                render_line(
                    frame,
                    area,
                    &mut row,
                    Line::styled(format!("{marker}{}", action.label()), style),
                );
            }
        }
    }
    if let Some(problem) = &state.problem {
        line(frame, area, &mut row, problem, WARNING);
    }
}

fn line(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    text: &str,
    color: ratatui::style::Color,
) {
    render_line(
        frame,
        area,
        row,
        Line::styled(text.to_owned(), Style::default().fg(color)),
    );
}

fn render_line(frame: &mut Frame<'_>, area: Rect, row: &mut u16, text: Line<'static>) {
    if *row < area.height {
        frame.render_widget(
            Paragraph::new(text),
            Rect::new(area.x, area.y + *row, area.width, 1),
        );
        *row += 1;
    }
}

fn field(frame: &mut Frame<'_>, area: Rect, row: &mut u16, editor: &mut Editor) {
    let width = usize::from(area.width.saturating_sub(3));
    let view = editor.project(width, 1);
    let line = view.lines.first().cloned().unwrap_or_default();
    render_line(
        frame,
        area,
        row,
        Line::from(vec![Span::raw("> "), Span::raw(line)]),
    );
    if *row > 0 {
        frame.set_cursor_position((area.x + 2 + view.cursor.0 as u16, area.y + *row - 1));
    }
}

pub(super) fn masked_field(frame: &mut Frame<'_>, area: Rect, row: &mut u16, editor: &Editor) {
    let mut masked = Editor::default();
    masked.set_text("•".repeat(editor.text().chars().count()));
    field(frame, area, row, &mut masked);
}

fn shortened(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    let length = display.chars().count();
    if length <= 48 {
        display
    } else {
        let suffix = display.chars().rev().take(47).collect::<String>();
        format!("…{}", suffix.chars().rev().collect::<String>())
    }
}

struct TerminalRestore {
    raw: bool,
    paste: bool,
    alternate: bool,
}

impl TerminalRestore {
    fn new() -> io::Result<Self> {
        use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
        let mut restore = Self {
            raw: false,
            paste: false,
            alternate: false,
        };
        enable_raw_mode()?;
        restore.raw = true;
        execute!(io::stdout(), crossterm::event::EnableBracketedPaste)?;
        restore.paste = true;
        execute!(io::stdout(), EnterAlternateScreen)?;
        restore.alternate = true;
        Ok(restore)
    }
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        use crossterm::cursor::Show;
        use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
        let mut output = io::stdout();
        if self.paste {
            let _ = execute!(output, crossterm::event::DisableBracketedPaste);
        }
        let _ = execute!(output, Show);
        if self.alternate {
            let _ = execute!(output, LeaveAlternateScreen);
        }
        if self.raw {
            let _ = disable_raw_mode();
        }
        let _ = output.flush();
    }
}
