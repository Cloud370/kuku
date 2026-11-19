use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use kuku_client::ConnectionOptions;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};
use tempfile::NamedTempFile;
use toml_edit::{value, DocumentMut, Item, Table};

use crate::editor::Editor;
use crate::theme::{emphasis, ACCENT, ERROR, MUTED, SUCCESS, WARNING};

const TIERS: [&str; 3] = ["strong", "balanced", "light"];
const MIN_SETUP_WIDTH: u16 = 60;
const MIN_SETUP_HEIGHT: u16 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderPreset {
    Anthropic,
    OpenAi,
    DeepSeek,
    OpenRouter,
}

impl ProviderPreset {
    const ALL: [Self; 4] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::DeepSeek,
        Self::OpenRouter,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::DeepSeek => "DeepSeek official",
            Self::OpenRouter => "OpenRouter",
        }
    }

    fn connection(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-sonnet-4-6",
            Self::OpenAi => "gpt-5.4-mini",
            Self::DeepSeek => "deepseek-flash",
            Self::OpenRouter => "openai/gpt-6-luna",
        }
    }

    fn default_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    fn format(self) -> &'static str {
        match self {
            Self::Anthropic | Self::DeepSeek => "anthropic",
            Self::OpenAi => "openai-responses",
            Self::OpenRouter => "openai-chat",
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::DeepSeek => "https://api.deepseek.com/anthropic",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
        }
    }

    fn profile(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupStep {
    Provider,
    Model,
    Credential,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finish {
    SaveAndContinue,
    SaveAndExit,
    Cancel,
}

struct SetupState {
    provider: ProviderPreset,
    step: SetupStep,
    model: Editor,
    credential: Editor,
    review_selection: usize,
    finish: Option<Finish>,
    problem: Option<String>,
}

impl Default for SetupState {
    fn default() -> Self {
        let provider = ProviderPreset::Anthropic;
        let mut model = Editor::default();
        model.set_text(provider.default_model().to_owned());
        let mut credential = Editor::default();
        credential.set_text(provider.default_env().to_owned());
        Self {
            provider,
            step: SetupStep::Provider,
            model,
            credential,
            review_selection: 0,
            finish: None,
            problem: None,
        }
    }
}

impl SetupState {
    fn handle(&mut self, event: Event, credential_set: bool) {
        if self.finish.is_some() {
            return;
        }
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key, credential_set),
            Event::Paste(text) => {
                if let Some(editor) = self.editor_mut() {
                    editor.insert(&text);
                    self.problem = None;
                }
            }
            _ => {}
        }
    }

    fn handle_with_size(&mut self, event: Event, credential_set: bool, usable: bool) {
        if usable {
            self.handle(event, credential_set);
            return;
        }
        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Release
                && (key.code == KeyCode::Esc
                    || (key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')))
            {
                self.finish = Some(Finish::Cancel);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, credential_set: bool) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.finish = Some(Finish::Cancel);
            return;
        }
        if key.code == KeyCode::Esc {
            self.back();
            return;
        }
        if key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.previous();
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('a') {
            if let Some(editor) = self.editor_mut() {
                editor.set_text(String::new());
                self.problem = None;
            }
            return;
        }
        match self.step {
            SetupStep::Provider => match key.code {
                KeyCode::Up => self.change_provider(-1),
                KeyCode::Down => self.change_provider(1),
                KeyCode::Tab | KeyCode::Enter => self.advance(credential_set),
                _ => {}
            },
            SetupStep::Model | SetupStep::Credential => match key.code {
                KeyCode::Left => self.editor_mut().expect("field step").left(),
                KeyCode::Right => self.editor_mut().expect("field step").right(),
                KeyCode::Home => self.editor_mut().expect("field step").home(),
                KeyCode::End => self.editor_mut().expect("field step").end(),
                KeyCode::Backspace => self.editor_mut().expect("field step").backspace(),
                KeyCode::Delete => self.editor_mut().expect("field step").delete(),
                KeyCode::Tab | KeyCode::Enter => self.advance(credential_set),
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.editor_mut()
                        .expect("field step")
                        .insert(&character.to_string());
                    self.problem = None;
                }
                _ => {}
            },
            SetupStep::Review => match key.code {
                KeyCode::Up | KeyCode::Left => self.move_review(-1, credential_set),
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                    self.move_review(1, credential_set)
                }
                KeyCode::Enter => self.choose_review(credential_set),
                _ => {}
            },
        }
    }

    fn editor_mut(&mut self) -> Option<&mut Editor> {
        match self.step {
            SetupStep::Model => Some(&mut self.model),
            SetupStep::Credential => Some(&mut self.credential),
            SetupStep::Provider | SetupStep::Review => None,
        }
    }

    fn advance(&mut self, credential_set: bool) {
        self.problem = None;
        self.step = match self.step {
            SetupStep::Provider => SetupStep::Model,
            SetupStep::Model => {
                if let Err(error) = valid_model_id(self.model.text()) {
                    self.problem = Some(error);
                    return;
                }
                SetupStep::Credential
            }
            SetupStep::Credential => {
                if let Err(error) = valid_env_name(self.credential.text()) {
                    self.problem = Some(error);
                    return;
                }
                self.review_selection = 0;
                SetupStep::Review
            }
            SetupStep::Review => {
                self.choose_review(credential_set);
                return;
            }
        };
    }

    fn previous(&mut self) {
        self.problem = None;
        self.step = match self.step {
            SetupStep::Provider => return,
            SetupStep::Model => SetupStep::Provider,
            SetupStep::Credential => SetupStep::Model,
            SetupStep::Review => SetupStep::Credential,
        };
    }

    fn back(&mut self) {
        if self.step == SetupStep::Provider {
            self.finish = Some(Finish::Cancel);
        } else {
            self.previous();
        }
    }

    fn change_provider(&mut self, direction: isize) {
        let current = ProviderPreset::ALL
            .iter()
            .position(|provider| *provider == self.provider)
            .expect("selected provider");
        let next =
            (current as isize + direction).rem_euclid(ProviderPreset::ALL.len() as isize) as usize;
        self.provider = ProviderPreset::ALL[next];
        self.model
            .set_text(self.provider.default_model().to_owned());
        self.credential
            .set_text(self.provider.default_env().to_owned());
        self.problem = None;
    }

    fn review_count(credential_set: bool) -> usize {
        if credential_set {
            4
        } else {
            3
        }
    }

    fn move_review(&mut self, direction: isize, credential_set: bool) {
        let count = Self::review_count(credential_set) as isize;
        self.review_selection =
            (self.review_selection as isize + direction).rem_euclid(count) as usize;
    }

    fn choose_review(&mut self, credential_set: bool) {
        self.finish = match (credential_set, self.review_selection) {
            (true, 0) => Some(Finish::SaveAndContinue),
            (true, 1) => Some(Finish::SaveAndExit),
            (true, 2) => {
                self.previous();
                None
            }
            (true, _) => Some(Finish::Cancel),
            (false, 0) => Some(Finish::SaveAndExit),
            (false, 1) => {
                self.previous();
                None
            }
            (false, _) => Some(Finish::Cancel),
        };
    }

    fn credential_name(&self) -> Result<String, String> {
        valid_env_name(self.credential.text())
    }
}

pub(crate) async fn run_if_needed(options: &ConnectionOptions) -> io::Result<bool> {
    let Some(config_path) = missing_local_config(options) else {
        return Ok(true);
    };
    let Ok(service) = options.local_service() else {
        return Ok(true);
    };
    if !status_allows_setup(service.status().await) {
        return Ok(true);
    }
    let workspace = options.workspace().unwrap_or_else(|_| {
        options
            .workspace
            .clone()
            .unwrap_or_else(|| env::current_dir().unwrap_or_default())
    });
    let (finish, state) = run_setup(&config_path, &workspace)?;
    match finish {
        Finish::SaveAndContinue => Ok(true),
        Finish::SaveAndExit => {
            let credential = state.credential_name().map_err(io::Error::other)?;
            println!(
                "Saved config: {}\nSet {credential} in the shell that launches kuku, then restart kuku. The credential was not checked.",
                config_path.display()
            );
            Ok(false)
        }
        Finish::Cancel => {
            println!("First-run setup canceled. kuku did not write a config.");
            Ok(false)
        }
    }
}

fn status_allows_setup<T, E>(status: Result<Option<T>, E>) -> bool {
    matches!(status, Ok(None))
}

fn missing_local_config(options: &ConnectionOptions) -> Option<PathBuf> {
    if options.remote.is_some() || options.token.is_some() {
        return None;
    }
    let config_path = options.local_service().ok()?.config_path().to_owned();
    match fs::symlink_metadata(&config_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(config_path),
        _ => None,
    }
}

fn run_setup(config_path: &Path, workspace: &Path) -> io::Result<(Finish, SetupState)> {
    let mut state = SetupState::default();
    let _restore = TerminalRestore::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(io::Error::other)?;
    terminal.clear().map_err(io::Error::other)?;
    loop {
        let credential_set = state
            .credential_name()
            .ok()
            .is_some_and(|name| credential_is_set(&name));
        terminal
            .draw(|frame| render(frame, workspace, config_path, &mut state, credential_set))
            .map_err(io::Error::other)?;
        if let Some(finish) = state.finish.take() {
            if finish != Finish::Cancel {
                let size = terminal.size()?;
                if !setup_is_usable(size.width, size.height) {
                    continue;
                }
            }
            if let Err(error) = persist_setup(config_path, &state, finish) {
                state.problem = Some(format!("Could not save config: {error}"));
                state.step = SetupStep::Review;
                state.review_selection = 0;
                continue;
            }
            return Ok((finish, state));
        }
        let event = event::read()?;
        let credential_set = state
            .credential_name()
            .ok()
            .is_some_and(|name| credential_is_set(&name));
        let size = terminal.size()?;
        state.handle_with_size(
            event,
            credential_set,
            setup_is_usable(size.width, size.height),
        );
    }
}

fn persist_setup(path: &Path, state: &SetupState, finish: Finish) -> io::Result<()> {
    if finish == Finish::Cancel {
        return Ok(());
    }
    let contents = build_config(state.provider, state.model.text(), state.credential.text())?;
    write_config_exclusive(path, &contents)
}

fn build_config(provider: ProviderPreset, model: &str, credential: &str) -> io::Result<String> {
    let model = valid_model_id(model).map_err(invalid_input)?;
    let credential = valid_env_name(credential).map_err(invalid_input)?;
    let base = include_str!("../../kuku/assets/default-config.toml").replacen(
        "# Generated by kuku init.",
        "# Generated by kuku first-run setup.",
        1,
    );
    let mut document: DocumentMut = base.parse().map_err(invalid_input)?;
    document["default_model"] = value("balanced");
    let tiers = document["model"].as_table_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "default model tiers are missing",
        )
    })?;
    for tier_name in TIERS {
        let tier = tiers
            .get_mut(tier_name)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("default model tier '{tier_name}' is missing"),
                )
            })?;
        tier["provider"] = value(provider.connection());
        tier["model"] = value(model.as_str());
    }
    let providers = document["provider"].as_table_mut().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "default providers are missing")
    })?;
    providers.clear();
    let mut entry = Table::new();
    entry["format"] = value(provider.format());
    entry["base_url"] = value(provider.base_url());
    entry["api_key"] = value(format!("${credential}"));
    entry["profile"] = value(provider.profile());
    providers.insert(provider.connection(), Item::Table(entry));
    Ok(document.to_string())
}

fn write_config_exclusive(path: &Path, contents: &str) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "configuration path already exists; it was not overwritten",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(contents.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map(|_| ())
        .map_err(|error| error.error)
}

fn valid_model_id(input: &str) -> Result<String, String> {
    let model = input.trim();
    if model.is_empty() {
        return Err("Enter a model ID.".into());
    }
    if model.len() > 256
        || model
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("Model IDs must be a single line of at most 256 bytes.".into());
    }
    if model.starts_with('$') {
        return Err("Enter a literal model ID, not an environment-variable reference.".into());
    }
    Ok(model.to_owned())
}

fn env_value_is_nonempty(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

fn credential_is_set(name: &str) -> bool {
    env_value_is_nonempty(env::var_os(name).as_deref())
}

fn valid_env_name(input: &str) -> Result<String, String> {
    let name = input.trim();
    let mut characters = name.chars();
    if !characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err("Use a portable environment-variable name, such as ANTHROPIC_API_KEY.".into());
    }
    Ok(name.to_owned())
}

fn invalid_input(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
}

fn render(
    frame: &mut Frame<'_>,
    workspace: &Path,
    config_path: &Path,
    state: &mut SetupState,
    credential_set: bool,
) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" kuku · first-run setup ")
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if !setup_is_usable(area.width, area.height) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("Terminal too small.", emphasis(WARNING)),
                Line::raw("Minimum: 60 columns"),
                Line::raw("by 20 rows."),
                Line::raw("Resize to continue."),
                Line::raw("Esc / Ctrl+C cancels."),
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
            Line::from(format!("Workspace: {}", workspace.display())),
            Line::from(format!("Config: {}", config_path.display())),
        ]),
        chunks[0],
    );
    render_step(frame, chunks[1], state, credential_set);
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

fn setup_is_usable(width: u16, height: u16) -> bool {
    width >= MIN_SETUP_WIDTH && height >= MIN_SETUP_HEIGHT
}

fn render_step(frame: &mut Frame<'_>, area: Rect, state: &mut SetupState, credential_set: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut row = 0;
    match state.step {
        SetupStep::Provider => {
            render_line(
                frame,
                area,
                &mut row,
                Line::styled("Step 1/4 · Choose a provider", emphasis(ACCENT)),
            );
            render_line(
                frame,
                area,
                &mut row,
                Line::raw("Choose the service that will receive your requests."),
            );
            for provider in ProviderPreset::ALL {
                let selected = provider == state.provider;
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
                    Line::styled(format!("{marker}{}", provider.label()), style),
                );
            }
        }
        SetupStep::Model => {
            render_line(
                frame,
                area,
                &mut row,
                Line::styled("Step 2/4 · Model ID", emphasis(ACCENT)),
            );
            render_line(
                frame,
                area,
                &mut row,
                Line::raw("Edit the suggested ID; setup does not verify model availability."),
            );
            render_field(frame, area, &mut row, &mut state.model);
        }
        SetupStep::Credential => {
            render_line(
                frame,
                area,
                &mut row,
                Line::styled(
                    "Step 3/4 · Credential environment variable",
                    emphasis(ACCENT),
                ),
            );
            render_line(
                frame,
                area,
                &mut row,
                Line::raw(
                    "Enter a variable name only. kuku never displays or stores key values here.",
                ),
            );
            render_field(frame, area, &mut row, &mut state.credential);
            let status = match valid_env_name(state.credential.text()) {
                Ok(name) if credential_set => {
                    format!("{name} is set (credential not validated).")
                }
                Ok(name) => format!("{name} is not set. Set it in the shell that launches kuku."),
                Err(_) => "Enter a valid environment-variable name.".into(),
            };
            render_line(
                frame,
                area,
                &mut row,
                Line::styled(status, if credential_set { SUCCESS } else { WARNING }),
            );
        }
        SetupStep::Review => {
            render_line(
                frame,
                area,
                &mut row,
                Line::styled("Step 4/4 · Review", emphasis(ACCENT)),
            );
            render_line(
                frame,
                area,
                &mut row,
                Line::raw(format!("Provider: {}", state.provider.label())),
            );
            render_line(
                frame,
                area,
                &mut row,
                Line::raw(format!("Model: {}", state.model.text().trim())),
            );
            let credential = state.credential.text().trim();
            render_line(
                frame,
                area,
                &mut row,
                Line::raw(format!("Credential variable: {credential}")),
            );
            let status = if credential_set {
                format!("{credential} is set; the credential was not validated.")
            } else {
                format!("{credential} is not set; set it in the shell that launches kuku.")
            };
            render_line(
                frame,
                area,
                &mut row,
                Line::styled(status, if credential_set { SUCCESS } else { WARNING }),
            );
            render_line(
                frame,
                area,
                &mut row,
                Line::styled(
                    "No model request is made by setup.",
                    Style::default().fg(MUTED),
                ),
            );
            let choices: &[&str] = if credential_set {
                &["Save and continue", "Save and exit", "Back", "Cancel"]
            } else {
                &["Save and exit", "Back", "Cancel"]
            };
            state.review_selection = state.review_selection.min(choices.len() - 1);
            for (index, choice) in choices.iter().enumerate() {
                let marker = if index == state.review_selection {
                    "› "
                } else {
                    "  "
                };
                render_line(
                    frame,
                    area,
                    &mut row,
                    Line::raw(format!("{marker}{choice}")),
                );
            }
        }
    }
    if let Some(problem) = state.problem.as_deref() {
        render_line(
            frame,
            area,
            &mut row,
            Line::styled(problem.to_owned(), Style::default().fg(ERROR)),
        );
    }
}

fn render_line(frame: &mut Frame<'_>, area: Rect, row: &mut u16, content: Line<'static>) {
    if *row < area.height {
        frame.render_widget(
            Paragraph::new(content),
            Rect::new(area.x, area.y + *row, area.width, 1),
        );
        *row += 1;
    }
}

fn render_field(frame: &mut Frame<'_>, area: Rect, row: &mut u16, editor: &mut Editor) {
    if *row >= area.height {
        return;
    }
    let field_area = Rect::new(area.x, area.y + *row, area.width, 1);
    let text_width = usize::from(area.width.saturating_sub(2)).max(1);
    let view = editor.project(text_width, 1);
    let text = view.lines.into_iter().next().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(ACCENT)),
            Span::raw(text),
        ])),
        field_area,
    );
    let cursor = (2 + view.cursor.0).min(usize::from(area.width.saturating_sub(1))) as u16;
    frame.set_cursor_position((area.x + cursor, field_area.y));
    *row += 1;
}

struct TerminalRestore {
    raw: bool,
    alternate: bool,
    paste: bool,
    hidden: bool,
}

impl TerminalRestore {
    fn new() -> io::Result<Self> {
        let mut restore = Self {
            raw: false,
            alternate: false,
            paste: false,
            hidden: false,
        };
        enable_raw_mode()?;
        restore.raw = true;
        let mut stdout = io::stdout();
        restore.alternate = true;
        execute!(stdout, EnterAlternateScreen)?;
        restore.paste = true;
        execute!(stdout, EnableBracketedPaste)?;
        restore.hidden = true;
        execute!(stdout, Hide)?;
        Ok(restore)
    }
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.paste {
            let _ = execute!(stdout, DisableBracketedPaste);
        }
        if self.hidden {
            let _ = execute!(stdout, Show);
        }
        if self.alternate {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        if self.raw {
            let _ = disable_raw_mode();
        }
    }
}

#[cfg(test)]
#[path = "onboarding_tests.rs"]
mod tests;
