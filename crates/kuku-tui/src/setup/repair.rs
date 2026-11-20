use std::fmt;
use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::editor::Editor;
use crate::jobs::Selection;
use crate::theme::{ACCENT, MUTED, SUCCESS, WARNING};

use super::repair_backend::{
    CredentialRepairApplied, CredentialRepairPlan, CredentialRepairRequest, FailedRoute,
    PreparedCredentialRepair, SavedCredentialRepair,
};
use super::state::valid_stored_key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Preparing,
    Input,
    Confirm,
    Saving,
    SavedNotApplied,
    Restarting,
    Reconnecting,
    ReconnectFailed,
    Ready,
    Guidance,
    Failed,
}

pub(crate) struct CredentialRepair {
    pub(crate) ticket: u64,
    pub(crate) selection: Selection,
    pub(crate) request: CredentialRepairRequest,
    stage: Stage,
    plan: Option<Arc<CredentialRepairPlan>>,
    saved: Option<SavedCredentialRepair>,
    route: Option<FailedRoute>,
    key: Editor,
    problem: Option<String>,
    usable: bool,
}

impl fmt::Debug for CredentialRepair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialRepair")
            .field("stage", &self.stage)
            .field("credential", &"<redacted>")
            .finish()
    }
}

pub(crate) enum CredentialRepairAction {
    None,
    Close,
    Save(Arc<CredentialRepairPlan>, String),
    Restart(SavedCredentialRepair),
    Reconnect,
}

impl CredentialRepair {
    pub(crate) fn new(ticket: u64, selection: Selection, request: CredentialRepairRequest) -> Self {
        Self {
            ticket,
            selection,
            request,
            stage: Stage::Preparing,
            plan: None,
            saved: None,
            route: None,
            key: Editor::default(),
            problem: None,
            usable: false,
        }
    }

    pub(crate) fn prepared(&mut self, result: Result<PreparedCredentialRepair, String>) {
        if self.stage != Stage::Preparing {
            return;
        }
        match result {
            Ok(PreparedCredentialRepair::Local(plan)) => {
                self.route = Some(plan.route.clone());
                self.plan = Some(plan);
                self.stage = Stage::Input;
            }
            Ok(PreparedCredentialRepair::Remote(route)) => {
                self.route = Some(route);
                self.stage = Stage::Guidance;
            }
            Err(problem) => {
                self.problem = Some(problem);
                self.stage = Stage::Failed;
            }
        }
    }

    pub(crate) fn saved(&mut self, result: Result<CredentialRepairApplied, String>) -> bool {
        if !matches!(self.stage, Stage::Saving | Stage::Restarting) {
            return false;
        }
        self.key.take_text();
        match result {
            Ok(CredentialRepairApplied::Applied) => {
                self.plan = None;
                self.saved = None;
                self.problem = None;
                self.stage = Stage::Reconnecting;
                true
            }
            Ok(CredentialRepairApplied::SavedNotApplied(saved, problem)) => {
                self.plan = None;
                self.saved = Some(saved);
                self.problem = Some(problem);
                self.stage = Stage::SavedNotApplied;
                false
            }
            Err(problem) => {
                self.problem = Some(problem);
                self.stage = Stage::Input;
                false
            }
        }
    }

    pub(crate) fn reconnected(&mut self) {
        if self.stage == Stage::Reconnecting {
            self.stage = Stage::Ready;
        }
    }

    pub(crate) fn reconnect_failed(&mut self) {
        if self.stage == Stage::Reconnecting {
            self.stage = Stage::ReconnectFailed;
            self.problem = Some("The service restarted, but reconnecting failed. Reconnect before choosing Continue; no request was retried.".into());
        }
    }

    pub(crate) fn matches_selection(&self, selection: &Selection) -> bool {
        if matches!(
            self.stage,
            Stage::Reconnecting | Stage::ReconnectFailed | Stage::Ready
        ) {
            self.request.target == selection.target
        } else {
            self.stage == Stage::Failed || self.selection == *selection
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.key.take_text();
        self.plan = None;
        self.saved = None;
        self.stage = Stage::Failed;
        self.problem = Some("The service or selected Session changed. Return and inspect its current state before repairing credentials. No retry was sent.".into());
    }

    pub(crate) fn handle(&mut self, event: Event) -> CredentialRepairAction {
        if matches!(
            self.stage,
            Stage::Saving | Stage::Restarting | Stage::Reconnecting
        ) {
            return CredentialRepairAction::None;
        }
        if let Event::Paste(text) = event {
            if self.stage == Stage::Input {
                self.key.insert(&text);
            }
            return CredentialRepairAction::None;
        }
        let Event::Key(key) = event else {
            return CredentialRepairAction::None;
        };
        if key.kind == KeyEventKind::Release {
            return CredentialRepairAction::None;
        }
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return CredentialRepairAction::Close;
        }
        if key.code == KeyCode::Enter {
            if !self.usable && matches!(self.stage, Stage::Input | Stage::Confirm) {
                self.problem =
                    Some("Resize to at least 60 columns by 20 rows before saving.".into());
                return CredentialRepairAction::None;
            }
            return match self.stage {
                Stage::Input => {
                    match valid_stored_key(self.key.text()) {
                        Ok(_) => {
                            self.problem = None;
                            self.stage = Stage::Confirm;
                        }
                        Err(_) => {
                            self.problem = Some(
                                "Enter a new key as a single line of at most 4096 bytes.".into(),
                            )
                        }
                    }
                    CredentialRepairAction::None
                }
                Stage::Confirm => {
                    let Some(plan) = self.plan.clone() else {
                        return CredentialRepairAction::None;
                    };
                    let Ok(key) = valid_stored_key(&self.key.take_text()) else {
                        return CredentialRepairAction::None;
                    };
                    self.stage = Stage::Saving;
                    CredentialRepairAction::Save(plan, key)
                }
                Stage::SavedNotApplied => {
                    let Some(saved) = self.saved.clone() else {
                        return CredentialRepairAction::None;
                    };
                    self.stage = Stage::Restarting;
                    CredentialRepairAction::Restart(saved)
                }
                Stage::ReconnectFailed => {
                    self.problem = None;
                    self.stage = Stage::Reconnecting;
                    CredentialRepairAction::Reconnect
                }
                Stage::Ready | Stage::Guidance | Stage::Failed => CredentialRepairAction::Close,
                Stage::Preparing | Stage::Saving | Stage::Restarting | Stage::Reconnecting => {
                    CredentialRepairAction::None
                }
            };
        }
        if self.stage == Stage::Input {
            match key.code {
                KeyCode::Backspace => self.key.backspace(),
                KeyCode::Delete => self.key.delete(),
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.key.take_text();
                }
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.key.insert(&character.to_string())
                }
                _ => {}
            }
        }
        CredentialRepairAction::None
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Repair provider credentials")
            .border_style(Style::default().fg(ACCENT));
        let inner = block.inner(frame.area());
        frame.render_widget(block, frame.area());
        self.usable = super::ui::usable(frame.area().width, frame.area().height);
        if !self.usable {
            frame.render_widget(Paragraph::new("Terminal too small. Resize to at least 60 columns by 20 rows. Esc cancels before saving."), inner);
            return;
        }
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(inner);
        let mut header = vec![Line::raw(format!(
            "Agent: {} · existing Session",
            self.request.target.agent
        ))];
        if let Some(route) = &self.route {
            header.push(Line::raw(format!(
                "Failed connection/model: {}/{}",
                route.connection, route.model
            )));
        }
        if let Some(plan) = &self.plan {
            header.push(Line::raw(format!("Config: {}", plan.config_path.display())));
        }
        frame.render_widget(Paragraph::new(header), chunks[0]);
        let text = match self.stage {
            Stage::Preparing => "Reading the failed request and active service settings…",
            Stage::Input => "Paste a new key; every character is masked.\nSaved locally as plaintext, not encrypted. Only this connection's credential will change.",
            Stage::Confirm => "Save credentials and restart local service\n\nThe current Session, Agent, draft and input remain. Other active work can defer restart. No model request will be made.",
            Stage::Saving => "Saving credentials and restarting the local service…\nNo request is being retried.",
            Stage::SavedNotApplied => "Credentials saved; service not restarted\n\nRestart local service when its other work has finished. The saved credential is not confirmed as applied.",
            Stage::Restarting => "Restarting the local service…\nNo request is being retried.",
            Stage::Reconnecting => "Credentials saved and local service restarted.\nReconnecting to the same Session…",
            Stage::ReconnectFailed => "Local service restarted; reconnect before retrying.\n\nEnter reconnects. Esc returns without sending a request.",
            Stage::Ready => "Credentials saved and local service restarted.\n\nReturn to the Session, then choose Continue to retry the existing input once. No retry has been sent.",
            Stage::Guidance => "Correct credentials on the service host; no local files changed.\n\n1. Run `kuku init` there with its config/data-dir options.\n   Or update the environment that launches that service.\n2. Run `kuku server restart` there when idle.\n3. Reconnect here, then choose Continue.\n\nThis client cannot write remote provider settings.",
            Stage::Failed => "Credential repair is unavailable for this view. No request was retried.",
        };
        let mut lines = text
            .lines()
            .map(|line| Line::raw(line.to_owned()))
            .collect::<Vec<_>>();
        if let Some(problem) = &self.problem {
            lines.push(Line::default());
            lines.extend(
                problem
                    .lines()
                    .map(|line| Line::styled(line.to_owned(), Style::default().fg(WARNING))),
            );
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[1]);
        if self.stage == Stage::Input {
            let mut row = 3;
            super::ui::masked_field(frame, chunks[1], &mut row, &self.key);
        }
        let footer = match self.stage {
            Stage::Input => "Enter reviews · Ctrl+A clears · Esc cancels without saving",
            Stage::Confirm => "Enter saves and restarts · Esc cancels without saving",
            Stage::SavedNotApplied => "Enter restarts local service · Esc returns without retrying",
            Stage::Saving | Stage::Restarting | Stage::Reconnecting => {
                "Waiting for the local service · No retry sent"
            }
            Stage::Ready => "Enter returns to Session · Continue is a separate action",
            Stage::ReconnectFailed => "Enter reconnects · Esc returns without retrying",
            Stage::Preparing => "Esc cancels without saving",
            Stage::Guidance | Stage::Failed => "Enter / Esc returns to Session",
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                footer,
                Style::default()
                    .fg(if self.stage == Stage::Ready {
                        SUCCESS
                    } else {
                        MUTED
                    })
                    .add_modifier(Modifier::BOLD),
            ))
            .wrap(Wrap { trim: false }),
            chunks[2],
        );
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    use super::{
        CredentialRepair, CredentialRepairAction, CredentialRepairApplied, CredentialRepairRequest,
        FailedRoute, PreparedCredentialRepair, Stage,
    };
    use crate::app::App;

    fn state(app: &App) -> CredentialRepair {
        let selection = app.selection();
        let request = CredentialRepairRequest {
            target: selection.target.clone(),
            turn: 7,
            request_id: "req-1".into(),
        };
        let mut state = CredentialRepair::new(1, selection, request);
        state.stage = Stage::Input;
        state.usable = true;
        state
    }

    #[test]
    fn secret_events_stay_masked_and_never_enter_the_composer_or_clipboard() {
        let mut app = App::new(std::env::current_dir().unwrap(), None);
        app.pane_mut().draft.set_text("preserved draft".into());
        app.effects.clear();
        let state = state(&app);
        app.credential_repair = Some(Box::new(state));
        let key = "fixture-repair-only-secret";
        app.event(
            Event::Paste(key.into()),
            &Buffer::empty(Rect::new(0, 0, 96, 24)),
        );
        assert_eq!("preserved draft", app.pane().draft.text());
        assert!(app.effects.is_empty());
        let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
        terminal
            .draw(|frame| app.credential_repair.as_mut().unwrap().render(frame))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains(&"•".repeat(key.len())));
        assert!(!screen.contains(key));
        assert!(!format!("{:?}", app.credential_repair).contains(key));
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            terminal.backend().buffer(),
        );
        assert!(app.credential_repair.is_none());
        assert_eq!("preserved draft", app.pane().draft.text());
        assert!(app.effects.is_empty());
    }

    #[test]
    fn repair_never_retries_and_requires_a_separate_save_confirmation() {
        let app = App::new(std::env::current_dir().unwrap(), None);
        let mut state = state(&app);
        state.handle(Event::Paste("fixture-only-key".into()));
        let enter = || Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            state.handle(enter()),
            CredentialRepairAction::None
        ));
        assert_eq!(Stage::Confirm, state.stage);
        assert!(matches!(
            state.handle(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))),
            CredentialRepairAction::Close
        ));
        state.stage = Stage::Saving;
        assert!(state.saved(Ok(CredentialRepairApplied::Applied)));
        state.reconnected();
        assert!(matches!(
            state.handle(enter()),
            CredentialRepairAction::Close
        ));
    }

    #[test]
    fn reconnect_failure_returns_control_without_retrying_or_losing_the_session() {
        let app = App::new(std::env::current_dir().unwrap(), None);
        let mut state = state(&app);
        state.stage = Stage::Saving;
        assert!(state.saved(Ok(CredentialRepairApplied::Applied)));
        state.reconnect_failed();
        assert_eq!(Stage::ReconnectFailed, state.stage);
        assert!(matches!(
            state.handle(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            CredentialRepairAction::Reconnect
        ));
        assert_eq!(Stage::Reconnecting, state.stage);
    }

    #[test]
    fn a_changed_selection_cannot_revive_repair_or_redirect_secret_input_to_a_draft() {
        let mut app = App::new(std::env::current_dir().unwrap(), None);
        let state = state(&app);
        let old = state.selection.clone();
        let request = state.request.clone();
        app.credential_repair = Some(Box::new(state));
        let original = app.active.clone();
        app.select(
            crate::app::Address::new(original.session.clone(), "reviewer".into()),
            true,
            String::new(),
        );
        app.select(original, true, String::new());
        app.update(crate::jobs::Update::PreparedCredentialRepair(
            1,
            old,
            Ok(PreparedCredentialRepair::Remote(FailedRoute {
                protocol: kuku::config::ProviderFormat::Anthropic,
                endpoint: "https://api.anthropic.com".parse().unwrap(),
                connection: "old".into(),
                model: "old".into(),
            })),
        ));
        let repair = app.credential_repair.as_ref().unwrap();
        assert_eq!(request, repair.request);
        assert_eq!(Stage::Failed, repair.stage);
        app.event(
            Event::Paste("fixture-must-stay-secret".into()),
            &Buffer::empty(Rect::new(0, 0, 96, 24)),
        );
        assert!(app.pane().draft.text().is_empty());
        assert!(!app.effects.iter().any(|job| matches!(
            job,
            crate::jobs::Job::Save(_) | crate::jobs::Job::SaveCredentialRepair { .. }
        )));
    }

    #[test]
    fn save_requires_the_form_to_be_visible_at_the_supported_terminal_size() {
        let app = App::new(std::env::current_dir().unwrap(), None);
        let mut state = state(&app);
        state.usable = false;
        state.handle(Event::Paste("fixture-only-key".into()));
        let enter = || Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.handle(enter());
        assert_eq!(Stage::Input, state.stage);
        let mut terminal = Terminal::new(TestBackend::new(96, 24)).unwrap();
        terminal.draw(|frame| state.render(frame)).unwrap();
        state.handle(enter());
        assert_eq!(Stage::Confirm, state.stage);
        let mut small = Terminal::new(TestBackend::new(40, 12)).unwrap();
        small.draw(|frame| state.render(frame)).unwrap();
        assert!(matches!(
            state.handle(enter()),
            CredentialRepairAction::None
        ));
        assert_eq!(Stage::Confirm, state.stage);
    }

    #[test]
    fn a_rejected_reconnection_never_marks_credentials_ready_for_the_session() {
        let workspace = std::env::current_dir().unwrap();
        let mut app = App::new(workspace.clone(), None);
        app.update(crate::jobs::Update::Connected {
            endpoint: "http://127.0.0.1:1".into(),
            workspace: workspace.clone(),
        });
        let mut repair = state(&app);
        repair.stage = Stage::Saving;
        assert!(repair.saved(Ok(CredentialRepairApplied::Applied)));
        app.credential_repair = Some(Box::new(repair));
        app.update(crate::jobs::Update::Connected {
            endpoint: "http://127.0.0.1:2".into(),
            workspace: workspace.join("changed-workspace"),
        });
        assert!(app.connection_problem.is_some());
        assert_eq!(
            Stage::ReconnectFailed,
            app.credential_repair.as_ref().unwrap().stage
        );
    }
}
