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
use super::models::{protocol_label, PROTOCOLS};
use super::state::{SetupIntent, SetupState, SetupStep};
use super::{CredentialSource, Finish, SetupMode};

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
            if let Some(connection) = state.take_discovery(settings) {
                terminal.draw(|frame| render(frame, settings, state, mode))?;
                let result = kuku::adapter::discover_models(&connection)
                    .await
                    .map_err(|error| error.to_string());
                state.discovery_finished(result);
            }
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
            line(frame, area, &mut row, "Connection", ACCENT);
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
            line(frame, area, &mut row, "Credential source", ACCENT);
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
            line(frame, area, &mut row, "Credential", ACCENT);
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
        SetupStep::Review => render_review(frame, area, &mut row, settings, state, mode),
        SetupStep::Protocol
        | SetupStep::Address
        | SetupStep::ModelsUrl
        | SetupStep::ModelSource
        | SetupStep::Discover
        | SetupStep::DefaultModel
        | SetupStep::Thinking
        | SetupStep::Output
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
    line(frame, area, row, "Review models", ACCENT);
    if let Ok(url) = state.request_url() {
        line(frame, area, row, &format!("Request · {url}"), MUTED);
    }
    let model = state.primary_model().unwrap_or_default();
    let count = state.models.selected.len().saturating_sub(1);
    let selection = if count > 0 {
        format!("{model} + {count} more")
    } else {
        model
    };
    let role = if state.intent == SetupIntent::Default {
        "Default model"
    } else {
        "Model"
    };
    line(frame, area, row, &format!("{role} · {selection}"), MUTED);
    let output = match state.output {
        kuku::config::RequestedOutput::Text => "text",
        kuku::config::RequestedOutput::Image => "image (adapter supports text)",
    };
    let unchanged_model = state.route_name(settings).and_then(|route| settings.resolved.tier(route))
        .zip(state.primary_model())
        .is_some_and(|(route, model)| route.model == model &&
            matches!(state.choice(), super::ConnectionChoice::Existing(name) if route.provider == *name));
    let effort = (unchanged_model || state.reasoning_changed)
        .then_some(state.reasoning.as_deref())
        .flatten()
        .filter(|effort| *effort != "provider-default");
    let thinking = effort.unwrap_or("Provider default");
    line(
        frame,
        area,
        row,
        &format!("Thinking · {thinking}    Output · {output}"),
        MUTED,
    );
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
            "Choose Paste API key to repair this credential."
        } else {
            "Storage unavailable; set the variable before launch."
        };
        line(frame, area, row, guidance, WARNING);
    }
    for (index, action) in state.review_choices(mode).iter().enumerate() {
        option(
            frame,
            area,
            row,
            action.label(),
            index == state.review_selection,
        );
    }
}

fn render_model_settings(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    settings: &Settings,
    state: &mut SetupState,
) {
    match state.step {
        SetupStep::Protocol => {
            line(frame, area, row, "API protocol", ACCENT);
            for format in PROTOCOLS {
                option(
                    frame,
                    area,
                    row,
                    protocol_label(format),
                    format == state.protocol,
                );
            }
        }
        SetupStep::Address => {
            let mode = if state.full_url {
                "Full request URL"
            } else {
                "Base URL"
            };
            line(frame, area, row, mode, ACCENT);
            if state.address.text().is_empty() && !state.full_url {
                let placeholder = state.default_base_url();
                line(frame, area, row, &format!("> {placeholder}"), MUTED);
                frame.set_cursor_position((area.x + 2, area.y + *row - 1));
            } else {
                field(frame, area, row, &mut state.address);
            }
            if let Ok(url) = state.request_url() {
                line(frame, area, row, &format!("Request · {url}"), MUTED);
            }
            line(
                frame,
                area,
                row,
                "F2 switch between Base URL and full request URL",
                MUTED,
            );
        }
        SetupStep::ModelsUrl => {
            line(frame, area, row, "Models URL (optional)", ACCENT);
            let (base, _) = state.address_parts();
            let options = state.connection_options(settings);
            field(frame, area, row, &mut state.models_url);
            if let Ok(url) = kuku::config::resolve_models_url(state.protocol, &base, &options) {
                line(frame, area, row, &format!("Discovery · {url}"), MUTED);
            } else {
                line(
                    frame,
                    area,
                    row,
                    "Enter a list URL to discover models at this custom endpoint.",
                    MUTED,
                );
            }
        }
        SetupStep::ModelSource => {
            line(frame, area, row, "Choose models", ACCENT);
            let (base, _) = state.address_parts();
            let options = state.connection_options(settings);
            if let Ok(url) = kuku::config::resolve_models_url(state.protocol, &base, &options) {
                line(
                    frame,
                    area,
                    row,
                    &format!("Read models using this credential · {url}"),
                    MUTED,
                );
            }
            if state.discovery_pending {
                line(frame, area, row, "Reading model list…", MUTED);
            } else {
                option(
                    frame,
                    area,
                    row,
                    "Enter model ID manually",
                    state.models.cursor == 0,
                );
                option(
                    frame,
                    area,
                    row,
                    "Discover models",
                    state.models.cursor == 1,
                );
                if !state.models.selected.is_empty() {
                    line(
                        frame,
                        area,
                        row,
                        &format!("{} selected", state.models.selected.len()),
                        MUTED,
                    );
                }
            }
        }
        SetupStep::Discover => {
            line(frame, area, row, "Discovered models", ACCENT);
            field(frame, area, row, &mut state.models.query);
            line(
                frame,
                area,
                row,
                &format!(
                    "{} selected · Space select · Tab continue · Esc back",
                    state.models.selected.len()
                ),
                MUTED,
            );
            let visible = usize::from(area.height.saturating_sub(*row + 1));
            let start = state
                .models
                .cursor
                .saturating_sub(visible.saturating_sub(1));
            for (index, model) in state
                .models
                .filtered()
                .enumerate()
                .skip(start)
                .take(visible)
            {
                let check = if state.models.selected.contains(&model.id) {
                    "✓"
                } else {
                    " "
                };
                option(
                    frame,
                    area,
                    row,
                    &format!("[{check}] {}", model.id),
                    index == state.models.cursor,
                );
            }
        }
        SetupStep::DefaultModel => {
            line(frame, area, row, "Choose the default model", ACCENT);
            let visible = usize::from(area.height.saturating_sub(*row + 1));
            let start = state
                .models
                .cursor
                .saturating_sub(visible.saturating_sub(1));
            for (index, model) in state
                .models
                .selected
                .iter()
                .enumerate()
                .skip(start)
                .take(visible)
            {
                option(frame, area, row, model, index == state.models.cursor);
            }
        }
        SetupStep::Thinking => {
            line(frame, area, row, "Thinking", ACCENT);
            if let Some(default) = state
                .capabilities(settings)
                .and_then(|caps| caps.reasoning_default)
            {
                line(
                    frame,
                    area,
                    row,
                    &format!(
                        "Upstream default · {} ({})",
                        default.value,
                        source_label(default.source)
                    ),
                    MUTED,
                );
            }
            for (index, effort) in state.thinking_choices(settings).iter().enumerate() {
                let label = match effort.as_str() {
                    "provider-default" => "Provider default · omit effort",
                    "none" => "Disable thinking · none",
                    _ => effort,
                };
                option(frame, area, row, label, index == state.models.cursor);
            }
        }
        SetupStep::Output => {
            line(frame, area, row, "Agent requested output", ACCENT);
            option(frame, area, row, "Text", state.models.cursor == 0);
            option(frame, area, row, "Image", state.models.cursor == 1);
            line(
                frame,
                area,
                row,
                "Built-in runtime supports text only.",
                MUTED,
            );
        }
        SetupStep::Advanced => {
            line(
                frame,
                area,
                row,
                "Manual model capability overrides",
                ACCENT,
            );
            let caps = state.capabilities(settings);
            let reasoning = caps
                .as_ref()
                .and_then(|caps| caps.reasoning_source)
                .map(|source| {
                    format!(
                        "{} ({})",
                        caps.as_ref()
                            .expect("known reasoning")
                            .reasoning_efforts
                            .join(", "),
                        source_label(source)
                    )
                });
            let default = caps
                .as_ref()
                .and_then(|caps| caps.reasoning_default.as_ref())
                .map(|value| format!("{} ({})", value.value, source_label(value.source)));
            let media = |value: Option<&kuku::DeclaredCapability<Vec<String>>>| {
                value.map(|value| {
                    format!(
                        "{} ({})",
                        value.value.join(", "),
                        source_label(value.source)
                    )
                })
            };
            let values = [
                reasoning,
                default,
                media(
                    caps.as_ref()
                        .and_then(|caps| caps.input_modalities.as_ref()),
                ),
                media(
                    caps.as_ref()
                        .and_then(|caps| caps.output_modalities.as_ref()),
                ),
                media(caps.as_ref().and_then(|caps| caps.interfaces.as_ref())),
            ];
            for (index, (label, value)) in ADVANCED_LABELS.iter().zip(values).enumerate() {
                let value = value.as_deref().unwrap_or("unknown");
                option(
                    frame,
                    area,
                    row,
                    &format!("{label} · {value}"),
                    index == state.models.cursor,
                );
            }
        }
        SetupStep::Override => {
            line(
                frame,
                area,
                row,
                ADVANCED_LABELS[state.models.cursor],
                ACCENT,
            );
            line(
                frame,
                area,
                row,
                "Manual values · comma-separated · blank uses declarations",
                MUTED,
            );
            field(frame, area, row, &mut state.models.override_value);
        }
        SetupStep::Connection
        | SetupStep::Model
        | SetupStep::CredentialSource
        | SetupStep::Credential
        | SetupStep::Review => unreachable!("handled by render_step"),
    }
}

const ADVANCED_LABELS: [&str; 5] = [
    "Native efforts",
    "Default effort",
    "Input modalities",
    "Output modalities",
    "Advertised interfaces",
];

fn source_label(source: kuku::CapabilitySource) -> &'static str {
    match source {
        kuku::CapabilitySource::ModelList => "model list",
        kuku::CapabilitySource::ProtocolModelList => "protocol model list",
        kuku::CapabilitySource::Manual => "manual",
        kuku::CapabilitySource::ProtocolManual => "protocol manual",
        kuku::CapabilitySource::AdapterProfile => "adapter profile",
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
