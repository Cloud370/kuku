use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};

use crate::editor::Editor;
use crate::text::{shorten_middle, single_line};
use crate::theme::{emphasis, focused_row, ACCENT, MUTED, SUCCESS, WARNING};

use super::config::Settings;
use super::state::{ReviewAction, SetupIntent, SetupState, SetupStep};
use super::{CredentialSource, Finish, SetupMode};

mod model_pages;

use model_pages::render_model_settings;

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 20;

pub(super) async fn run(
    settings: &Settings,
    state: &mut SetupState,
    mode: SetupMode,
) -> io::Result<Finish> {
    let _restore = TerminalRestore::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    loop {
        state.poll_probes(settings);
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
        if event::poll(Duration::ZERO)? {
            let event = event::read()?;
            let size = terminal.size()?;
            if !usable(size.width, size.height) {
                if handle_small_terminal_event(state, &event) {
                    return Ok(Finish::Cancel);
                }
                continue;
            }
            state.handle(event, settings, mode);
        } else {
            tokio::time::sleep(Duration::from_millis(35)).await;
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
        .title(" kuku · model setup ")
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
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    render_step(frame, chunks[0], settings, state, mode);
    let keys = if state.step == SetupStep::TrialResults {
        "↑/↓ scroll · Enter or Esc back"
    } else {
        "↑/↓ move · Enter choose · Esc back · Ctrl+C cancel"
    };
    frame.render_widget(
        Paragraph::new(keys).style(Style::default().fg(MUTED)),
        chunks[1],
    );
}

fn render_step(
    frame: &mut Frame<'_>,
    area: Rect,
    settings: &Settings,
    state: &mut SetupState,
    mode: SetupMode,
) {
    let area = if matches!(
        state.step,
        SetupStep::Review | SetupStep::ReviewSettings | SetupStep::TrialResults
    ) {
        area.inner(Margin::new(2, 1))
    } else {
        area
    };
    let mut row = 0;
    match state.step {
        SetupStep::Consent => {
            line(frame, area, &mut row, "Set up a model", ACCENT);
            line(frame, area, &mut row, "Tests may cost money.", WARNING);
            for (index, name) in ["Find and test models", "Edit config file"]
                .iter()
                .enumerate()
            {
                option(frame, area, &mut row, name, index == state.models.cursor);
            }
        }
        SetupStep::ManualConfig => {
            line(frame, area, &mut row, "Manual configuration", ACCENT);
            line(
                frame,
                area,
                &mut row,
                &settings.config_path.display().to_string(),
                MUTED,
            );
            line(
                frame,
                area,
                &mut row,
                "Edit the provider and model entries in config.toml.",
                MUTED,
            );
            line(
                frame,
                area,
                &mut row,
                "Run kuku config validate to check them. Enter exits.",
                MUTED,
            );
        }
        SetupStep::Connection => {
            line(frame, area, &mut row, "Choose a provider", ACCENT);
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
            line(frame, area, &mut row, "Model ID", ACCENT);
            line(
                frame,
                area,
                &mut row,
                &match &state.intent {
                    SetupIntent::Default => "Choose the default model ID".into(),
                    SetupIntent::Add => "Add a model to this connection".into(),
                    SetupIntent::Edit(tier) => format!("Editing {tier}"),
                },
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
            line(frame, area, &mut row, "API key source", ACCENT);
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
                "Keys are tested after you choose a model.",
                MUTED,
            );
            if !settings.can_store_key() {
                line(frame, area, &mut row, settings.storage_problem(), WARNING);
            }
        }
        SetupStep::Credential => {
            line(frame, area, &mut row, "API key", ACCENT);
            match state.credential_source {
                CredentialSource::Environment => {
                    line(frame, area, &mut row, "API key variable name", MUTED);
                    field(frame, area, &mut row, &mut state.environment);
                    let status = if state.environment_set() {
                        "Key found."
                    } else {
                        "Key missing. You can set it after saving."
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
                    line(frame, area, &mut row, "Paste your API key", MUTED);
                    key_preview_field(frame, area, &mut row, &state.stored_key);
                    line(
                        frame,
                        area,
                        &mut row,
                        "Saved on this device without encryption.",
                        WARNING,
                    );
                    if let Some(hint) = state.saved_key_hint(settings) {
                        line(frame, area, &mut row, hint, SUCCESS);
                    }
                }
            }
        }
        SetupStep::Review => render_review(frame, area, &mut row, settings, state, mode),
        SetupStep::ReviewSettings => {
            render_review_settings(frame, area, &mut row, settings, state, mode)
        }
        SetupStep::Protocol
        | SetupStep::Address
        | SetupStep::AddressKind
        | SetupStep::Network
        | SetupStep::ProxyAddress
        | SetupStep::ModelSource
        | SetupStep::Discover
        | SetupStep::TrialResults
        | SetupStep::DefaultModel
        | SetupStep::Thinking
        | SetupStep::Output
        | SetupStep::Pricing
        | SetupStep::Rates
        | SetupStep::Advanced
        | SetupStep::Override => render_model_settings(frame, area, &mut row, settings, state),
    }
    if let Some(problem) = &state.problem {
        line(frame, area, &mut row, problem, WARNING);
    }
}

fn render_review(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    settings: &Settings,
    state: &SetupState,
    mode: SetupMode,
) {
    render_line(
        frame,
        area,
        row,
        Line::styled("Save setup", emphasis(ACCENT)),
    );
    *row += 1;
    let model = state.primary_model().unwrap_or_default();
    let count = state.models.selected.len().saturating_sub(1);
    let selection = if count > 0 {
        format!("{model} + {count} more")
    } else {
        model
    };
    let role = if state.intent == SetupIntent::Default {
        "Default"
    } else {
        "Model"
    };
    summary_row(frame, area, row, role, &selection, None);
    if let Ok(url) = state.request_url() {
        summary_row(
            frame,
            area,
            row,
            "API URL",
            &super::probes::display_url(&url),
            None,
        );
    }
    if !state.trials.rows().is_empty() {
        let (progress, attention) = super::probes::review_status(&state.trials);
        summary_row(
            frame,
            area,
            row,
            "Tests",
            &progress,
            attention.then_some(WARNING),
        );
    }
    render_review_warnings(frame, area, row, settings, state, mode);
    *row += 1;
    review_menu(frame, area, row, state, mode, |_| None);
}

fn render_review_warnings(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    settings: &Settings,
    state: &SetupState,
    mode: SetupMode,
) {
    if let super::ConnectionChoice::Existing(name) = state.choice() {
        if state.connection(settings).is_ok_and(|connection| {
            settings.resolved.provider(name).is_some_and(|previous| {
                previous.format != connection.format
                    || previous.base_url != connection.base_url
                    || previous.options.endpoint_url != connection.options.endpoint_url
                    || previous.api_key != connection.api_key
            })
        }) {
            let shared = settings
                .resolved
                .tiers
                .iter()
                .filter(|(route, tier)| {
                    tier.provider == *name && state.route_name(settings) != Some(route)
                })
                .map(|(route, _)| route.as_str())
                .collect::<Vec<_>>();
            if !shared.is_empty() {
                line(
                    frame,
                    area,
                    row,
                    &format!("Also affects · {}", shared.join(", ")),
                    WARNING,
                );
            }
        }
    }
    if mode == SetupMode::FirstRun && !state.credential_ready() {
        let guidance = if settings.can_store_key() {
            "Paste an API key to continue."
        } else {
            "Set the API key variable before starting."
        };
        line(frame, area, row, guidance, WARNING);
    }
}

fn render_review_settings(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    settings: &Settings,
    state: &SetupState,
    mode: SetupMode,
) {
    render_line(frame, area, row, Line::styled("Settings", emphasis(ACCENT)));
    *row += 1;
    let unchanged_model = state.route_name(settings).and_then(|route| settings.resolved.tier(route))
        .zip(state.primary_model())
        .is_some_and(|(route, model)| route.model == model &&
            matches!(state.choice(), super::ConnectionChoice::Existing(name) if route.provider == *name));
    let effort = (unchanged_model || state.reasoning_changed)
        .then_some(state.reasoning.as_deref())
        .flatten()
        .filter(|effort| *effort != "provider-default");
    let output = match state.output {
        kuku::config::RequestedOutput::Text => "Text",
        kuku::config::RequestedOutput::Image => "Image requested",
    };
    review_menu(frame, area, row, state, mode, |action| match action {
        ReviewAction::Thinking => Some(effort.unwrap_or("Provider default")),
        ReviewAction::Output => Some(output),
        ReviewAction::Pricing => Some(state.pricing_label()),
        _ => None,
    });
}

fn summary_row(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    label: &str,
    value: &str,
    color: Option<ratatui::style::Color>,
) {
    const LABEL_WIDTH: usize = 10;
    let value = shorten_middle(
        &single_line(value),
        usize::from(area.width).saturating_sub(LABEL_WIDTH),
    );
    let style = color.map_or_else(Style::default, |color| Style::default().fg(color));
    render_line(
        frame,
        area,
        row,
        Line::from(vec![
            Span::styled(format!("{label:<LABEL_WIDTH$}"), Style::default().fg(MUTED)),
            Span::styled(value, style),
        ]),
    );
}

fn review_menu<'a>(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    state: &SetupState,
    mode: SetupMode,
    value: impl Fn(ReviewAction) -> Option<&'a str>,
) {
    for (index, action) in state.review_choices(mode).into_iter().enumerate() {
        if *row
            >= area
                .height
                .saturating_sub(u16::from(state.problem.is_some()))
        {
            break;
        }
        let selected = index == state.review_selection;
        let marker = if selected { "› " } else { "  " };
        let label = action.label();
        let text = match value(action) {
            Some(value) => format!("{marker}{label:<19} {}", single_line(value)),
            None => format!("{marker}{label}"),
        };
        let style = if selected {
            focused_row().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(text).style(style),
            Rect::new(area.x, area.y + *row, area.width, 1),
        );
        *row += 1;
    }
}

fn option(frame: &mut Frame<'_>, area: Rect, row: &mut u16, label: &str, selected: bool) {
    let marker = if selected { "› " } else { "  " };
    let style = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    render_line(
        frame,
        area,
        row,
        Line::styled(format!("{marker}{label}"), style),
    );
}

pub(super) fn line(
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

fn key_preview_field(frame: &mut Frame<'_>, area: Rect, row: &mut u16, editor: &Editor) {
    let mut preview = Editor::default();
    preview.set_text(mask_key(editor.text()));
    field(frame, area, row, &mut preview);
}

pub(super) fn mask_key(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    let (prefix, suffix) = match characters.len() {
        16.. => (7, 5),
        8..=15 => (2, 2),
        _ => (0, 0),
    };
    characters[..prefix]
        .iter()
        .copied()
        .chain(std::iter::repeat_n('•', characters.len() - prefix - suffix))
        .chain(characters[characters.len() - suffix..].iter().copied())
        .collect()
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
