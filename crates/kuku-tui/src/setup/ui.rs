use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::editor::Editor;
use crate::terminal::AuthorizationLink;
use crate::text::{shorten_middle, single_line};
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING, compact_padding, emphasis, focused_row};

use super::config::Settings;
use super::dashboard::TestOverview;
use super::state::{ReviewAction, SetupIntent, SetupState, SetupStep, StopTests};
use super::{CredentialSource, Finish, SetupMode};

mod model_pages;

use model_pages::render_model_settings;

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 20;
const MAX_CONTENT_WIDTH: u16 = 88;

pub(super) fn authorization_link(state: &SetupState, outer: Rect) -> Option<AuthorizationLink> {
    if state.step != SetupStep::SubscriptionSignIn || !usable(outer.width, outer.height) {
        return None;
    }
    state.enrollment.as_ref()?.link(outer)
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
    if is_cancel(event)
        && !state
            .enrollment
            .as_ref()
            .is_some_and(|flow| flow.verifying())
    {
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
    if usable(area.width, area.height)
        && state
            .enrollment
            .as_mut()
            .is_some_and(|flow| flow.render_authorization(frame, state.problem.as_deref()))
    {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" kuku · model setup ")
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if !usable(area.width, area.height) {
        let guidance = if mode == SetupMode::FirstRun {
            "F1 Configuration guide · Esc Exit"
        } else {
            "Esc / Ctrl+C cancels without saving."
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("Terminal too small.", Style::default().fg(WARNING)),
                Line::raw("Minimum: 60 columns by 20 rows."),
                Line::raw("Resize to continue."),
                Line::raw(guidance),
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
    let (escape, keys) = match state.step {
        SetupStep::Consent => ("Esc exit", " · ↑/↓ move · Enter choose"),
        SetupStep::SubscriptionSignIn
            if state
                .enrollment
                .as_ref()
                .is_some_and(|flow| flow.verifying()) =>
        {
            ("", "")
        }
        SetupStep::SubscriptionSignIn => ("Esc previous step", " · Ctrl+C exit"),
        SetupStep::TrialResults => ("Esc previous step", " · ↑/↓ scroll · Enter returns"),
        _ => (
            "Esc previous step",
            " · ↑/↓ move · Enter choose · Ctrl+C exit",
        ),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(escape, emphasis(ACCENT)),
            Span::styled(keys, Style::default().fg(MUTED)),
        ])),
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
    let mut area = if matches!(
        state.step,
        SetupStep::Review | SetupStep::ReviewSettings | SetupStep::TrialResults
    ) {
        Block::default()
            .padding(compact_padding(2, true))
            .inner(area)
    } else {
        area
    };
    area.width = area.width.min(MAX_CONTENT_WIDTH);
    let mut row = 0;
    match state.step {
        SetupStep::Consent => {
            line(frame, area, &mut row, "Set up a model", ACCENT);
            for (index, name) in ["Find and test models", "Edit config file"]
                .iter()
                .enumerate()
            {
                option(frame, area, &mut row, name, index == state.models.cursor);
            }
            if mode == SetupMode::FirstRun {
                line(frame, area, &mut row, "F1  Configuration guide", MUTED);
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
                "Set a default connection and model ID in config.toml.",
                MUTED,
            );
            line(
                frame,
                area,
                &mut row,
                "Run kuku config validate to check them. Enter exits.",
                MUTED,
            );
            if mode == SetupMode::FirstRun {
                line(frame, area, &mut row, "F1  Configuration guide", MUTED);
            }
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
        SetupStep::SubscriptionSignIn => {
            if let Some(flow) = &mut state.enrollment {
                flow.render(frame, area, &mut row);
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
                    SetupIntent::Edit(id) => format!("Editing {} · {}", id.model, id.connection),
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
            trial_usage(frame, area, &mut row, state);
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
        SetupStep::ConfirmStop(action) => {
            render_stop_confirmation(frame, area, &mut row, state, action)
        }
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
        | SetupStep::CheckingApi
        | SetupStep::TrialResults
        | SetupStep::RecoveryModels
        | SetupStep::RecoveryActions
        | SetupStep::RecoveryDefault
        | SetupStep::DefaultModel
        | SetupStep::Thinking
        | SetupStep::Service
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

fn trial_usage(frame: &mut Frame<'_>, area: Rect, row: &mut u16, state: &SetupState) {
    if state.authorized && state.subscription.is_none() {
        line(frame, area, row, "Model tests may use API balance.", MUTED);
    }
}

fn render_stop_confirmation(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    state: &SetupState,
    action: StopTests,
) {
    let testing = state.trials.pending();
    let (title, confirm) = match (action, testing) {
        (StopTests::Save, true) => ("Save before tests finish?", "Stop tests and save"),
        (StopTests::Save, false) => ("Tests finished", "Save setup"),
        (_, true) => ("Go back before tests finish?", "Stop tests and go back"),
        (_, false) => ("Tests finished", "Go back"),
    };
    line(frame, area, row, title, ACCENT);
    if testing {
        line(frame, area, row, "Pending tests will stop.", WARNING);
    }
    *row += 1;
    let stay = if testing {
        "Keep testing"
    } else {
        "Return to results"
    };
    option(frame, area, row, stay, state.models.cursor == 0);
    option(frame, area, row, confirm, state.models.cursor == 1);
}

fn render_review(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    settings: &Settings,
    state: &SetupState,
    mode: SetupMode,
) {
    let testing = state.trials.pending();
    let model = state.primary_model().unwrap_or_default();
    let models = state.model_ids().unwrap_or_default();
    let title = match (testing, models.len()) {
        (true, count) if count > 1 => format!("Testing models · {count} selected"),
        (false, count) if count > 1 => format!("Save setup · {count} models"),
        (true, _) => "Testing models".into(),
        (false, _) => "Save setup".into(),
    };
    render_line(frame, area, row, Line::styled(title, emphasis(ACCENT)));
    if area.height > MIN_HEIGHT - 2 {
        *row += 1;
    }
    if models.len() > 1 && state.intent == SetupIntent::Default {
        summary_row(frame, area, row, "Default", &model, None);
    }
    if state.subscription.is_some() {
        summary_row(
            frame,
            area,
            row,
            "Account",
            &state.connection_label(settings),
            None,
        );
    } else if let Ok(url) = state.request_url() {
        summary_row(
            frame,
            area,
            row,
            "API URL",
            &super::probes::display_url(&url),
            None,
        );
    }
    let overview = TestOverview::new(&state.trials, &models);
    if state.trials.selected_rows().next().is_some() {
        overview.render_progress(frame, area, row);
        let progress = super::probes::review_status(&state.trials);
        let color = testing.then_some(ACCENT);
        summary_row(frame, area, row, "Requests", &progress, color);
    }
    render_review_warnings(frame, area, row, settings, state, mode);
    let menu_rows = u16::try_from(state.review_choices(mode).len()).unwrap_or(area.height);
    let reserved = menu_rows + u16::from(state.problem.is_some());
    let room = area.height.saturating_sub(*row + reserved);
    let gap = u16::from(usize::from(room) > models.len().saturating_add(2));
    let connection = state.connection(settings).ok();
    overview.render_table(frame, area, row, connection.as_ref(), room - gap);
    *row += gap;
    let primary_failed = state.primary_failed();
    review_menu(frame, area, row, state, mode, |action| match action {
        ReviewAction::Save | ReviewAction::SaveAndExit if primary_failed => {
            Some("Keep unverified settings")
        }
        ReviewAction::TrialResults => super::probes::review_detail_hint(&state.trials),
        ReviewAction::FixModels => Some("Retry, skip or repair"),
        ReviewAction::UseAnotherDefault => Some("Choose a model that replied"),
        _ => None,
    });
}

fn render_review_warnings(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &mut u16,
    settings: &Settings,
    state: &SetupState,
    mode: SetupMode,
) {
    if state.text_only_route(settings) {
        line(
            frame,
            area,
            row,
            "Text only · For tools, choose Settings → API type → Responses",
            WARNING,
        );
    }
    if let super::ConnectionChoice::Existing(name) = state.choice() {
        if state.connection(settings).is_ok_and(|connection| {
            settings.provider(name).is_some_and(|previous| {
                previous.format != connection.format
                    || previous.base_url != connection.base_url
                    || previous.options.endpoint_url != connection.options.endpoint_url
                    || previous.api_key != connection.api_key
            })
        }) {
            let shared = settings
                .resolved
                .as_ref()
                .into_iter()
                .flat_map(|config| config.models.keys())
                .filter(|id| id.connection == *name && state.configured_model(settings) != Some(id))
                .map(|id| id.model.as_str())
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
    let effort = state.displayed_reasoning(settings);
    let effort = effort
        .as_deref()
        .filter(|effort| *effort != "provider-default");
    let output = match state.output {
        kuku::config::RequestedOutput::Text => "Text",
        kuku::config::RequestedOutput::Image => "Image requested",
    };
    let service = crate::text::service_tier_label(state.displayed_service(settings));
    review_menu(frame, area, row, state, mode, |action| match action {
        ReviewAction::Thinking => Some(effort.unwrap_or("Provider default")),
        ReviewAction::Service => Some(service.as_str()),
        ReviewAction::Output => Some(output),
        ReviewAction::Pricing => Some(state.pricing_label()),
        ReviewAction::TestModel | ReviewAction::TestTools if state.subscription.is_some() => {
            Some("Uses your ChatGPT plan")
        }
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

pub(super) fn field(frame: &mut Frame<'_>, area: Rect, row: &mut u16, editor: &mut Editor) {
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
