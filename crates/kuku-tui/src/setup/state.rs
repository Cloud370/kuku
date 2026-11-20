use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kuku::config::ApiKey;

use crate::editor::Editor;

use super::config::Settings;
use super::{ConnectionChoice, CredentialSource, Finish, SetupMode};

const MAX_MODEL_BYTES: usize = 256;
const MAX_KEY_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetupStep {
    Connection,
    Model,
    CredentialSource,
    Credential,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewAction {
    SaveAndContinue,
    SaveAndExit,
    RepairCredential,
    Back,
    Cancel,
}

impl ReviewAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SaveAndContinue => "Save and continue",
            Self::SaveAndExit => "Save and exit",
            Self::RepairCredential => "Paste API key",
            Self::Back => "Back",
            Self::Cancel => "Cancel",
        }
    }
}

pub(super) struct SetupState {
    pub(super) choices: Vec<ConnectionChoice>,
    pub(super) selected: usize,
    pub(super) step: SetupStep,
    pub(super) model: Editor,
    pub(super) environment: Editor,
    pub(super) environment_detected: bool,
    pub(super) stored_key: Editor,
    pub(super) credential_source: CredentialSource,
    pub(super) review_selection: usize,
    pub(super) finish: Option<Finish>,
    pub(super) problem: Option<String>,
    saved_key_available: bool,
    can_store_key: bool,
}

impl SetupState {
    pub(super) fn new(settings: &Settings) -> Self {
        let choices = settings.connection_choices();
        let selected = choices
            .iter()
            .position(|choice| *choice == settings.initial_choice())
            .unwrap_or(0);
        let mut state = Self {
            choices,
            selected,
            step: SetupStep::Connection,
            model: Editor::default(),
            environment: Editor::default(),
            environment_detected: false,
            stored_key: Editor::default(),
            credential_source: CredentialSource::Environment,
            review_selection: 0,
            finish: None,
            problem: None,
            saved_key_available: false,
            can_store_key: settings.can_store_key(),
        };
        state.load_selection(settings);
        state
    }

    pub(super) fn choice(&self) -> &ConnectionChoice {
        &self.choices[self.selected]
    }

    pub(super) fn connection_label(&self, settings: &Settings) -> String {
        settings.choice_label(self.choice())
    }

    pub(super) fn model_text(&self) -> Result<String, String> {
        valid_model_id(self.model.text())
    }

    pub(super) fn environment_name(&self) -> Option<String> {
        valid_env_name(self.environment.text()).ok()
    }

    pub(super) fn environment_set(&self) -> bool {
        self.environment_detected
    }

    pub(super) fn stored_key_is_entered(&self) -> bool {
        valid_stored_key(self.stored_key.text()).is_ok()
    }

    pub(super) fn stored_key_is_available(&self) -> bool {
        if self.stored_key.text().trim().is_empty() {
            self.saved_key_available
        } else {
            self.stored_key_is_entered()
        }
    }

    pub(super) fn credential_ready(&self) -> bool {
        match self.credential_source {
            CredentialSource::Environment => self.environment_set(),
            CredentialSource::StoredKey => self.stored_key_is_available(),
        }
    }

    pub(super) fn saved_key_hint(&self, settings: &Settings) -> Option<&'static str> {
        if !self.stored_key.text().is_empty() {
            return None;
        }
        if self.credential_source != CredentialSource::StoredKey || !self.saved_key_available {
            return None;
        }
        match settings.key_for(self.choice()) {
            Some(ApiKey::File(path)) if settings.file_reference_in_current_store(path) => {
                Some("Saved key configured; leave blank to keep.")
            }
            Some(ApiKey::File(_)) => Some(
                "Saved key configured; leave blank to move it into this data directory on save.",
            ),
            Some(ApiKey::Plaintext(_)) => {
                Some("Existing inline key will move to private storage when saved.")
            }
            _ => None,
        }
    }

    pub(super) fn handle(&mut self, event: Event, settings: &Settings, mode: SetupMode) {
        if self.finish.is_some() {
            return;
        }
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key, settings, mode),
            Event::Paste(text) => {
                match self.step {
                    SetupStep::Model => self.model.insert(&text),
                    SetupStep::Credential => match self.credential_source {
                        CredentialSource::Environment => self.environment.insert(&text),
                        CredentialSource::StoredKey => self.stored_key.insert(&text),
                    },
                    _ => return,
                }
                if self.step == SetupStep::Credential
                    && self.credential_source == CredentialSource::Environment
                {
                    self.refresh_environment_detection();
                }
                self.problem = None;
            }
            _ => {}
        }
    }

    fn key(&mut self, key: KeyEvent, settings: &Settings, mode: SetupMode) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.finish = Some(Finish::Cancel);
            return;
        }
        if key.code == KeyCode::Esc {
            if self.step == SetupStep::Connection {
                self.finish = Some(Finish::Cancel);
            } else {
                self.previous();
            }
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
            }
            if self.step == SetupStep::Credential
                && self.credential_source == CredentialSource::Environment
            {
                self.refresh_environment_detection();
            }
            self.problem = None;
            return;
        }
        match self.step {
            SetupStep::Connection => match key.code {
                KeyCode::Up | KeyCode::Left => self.change_choice(-1, settings),
                KeyCode::Down | KeyCode::Right => self.change_choice(1, settings),
                KeyCode::Tab | KeyCode::Enter => self.advance(settings),
                _ => {}
            },
            SetupStep::Model => self.edit_model(key),
            SetupStep::CredentialSource => match key.code {
                KeyCode::Up | KeyCode::Left | KeyCode::Down | KeyCode::Right => {
                    let source = match self.credential_source {
                        CredentialSource::Environment => CredentialSource::StoredKey,
                        CredentialSource::StoredKey => CredentialSource::Environment,
                    };
                    if source == CredentialSource::StoredKey
                        && !settings.can_store_key()
                        && !self.saved_key_available
                    {
                        self.problem = Some(settings.storage_problem().to_owned());
                        return;
                    }
                    self.credential_source = source;
                    if source == CredentialSource::Environment {
                        self.refresh_environment_detection();
                    }
                    self.review_selection = 0;
                    self.problem = None;
                }
                KeyCode::Tab | KeyCode::Enter => self.advance(settings),
                _ => {}
            },
            SetupStep::Credential => self.edit_credential(key, settings),
            SetupStep::Review => match key.code {
                KeyCode::Up | KeyCode::Left => self.move_review(-1, mode),
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => self.move_review(1, mode),
                KeyCode::Enter => self.choose_review(mode),
                _ => {}
            },
        }
    }

    fn edit_model(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left => self.model.left(),
            KeyCode::Right => self.model.right(),
            KeyCode::Home => self.model.home(),
            KeyCode::End => self.model.end(),
            KeyCode::Backspace => self.model.backspace(),
            KeyCode::Delete => self.model.delete(),
            KeyCode::Tab | KeyCode::Enter => self.advance_from_model(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.model.insert(&character.to_string());
                self.problem = None;
            }
            _ => {}
        }
    }

    fn edit_credential(&mut self, key: KeyEvent, settings: &Settings) {
        if matches!(key.code, KeyCode::Tab | KeyCode::Enter) {
            self.advance(settings);
            return;
        }
        let editor = match self.credential_source {
            CredentialSource::Environment => &mut self.environment,
            CredentialSource::StoredKey => &mut self.stored_key,
        };
        match key.code {
            KeyCode::Left if self.credential_source == CredentialSource::Environment => {
                editor.left()
            }
            KeyCode::Right if self.credential_source == CredentialSource::Environment => {
                editor.right()
            }
            KeyCode::Home if self.credential_source == CredentialSource::Environment => {
                editor.home()
            }
            KeyCode::End if self.credential_source == CredentialSource::Environment => editor.end(),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                editor.insert(&character.to_string());
                self.problem = None;
            }
            _ => {}
        }
        if self.credential_source == CredentialSource::Environment {
            self.refresh_environment_detection();
        }
    }

    fn editor_mut(&mut self) -> Option<&mut Editor> {
        match self.step {
            SetupStep::Model => Some(&mut self.model),
            SetupStep::Credential => Some(match self.credential_source {
                CredentialSource::Environment => &mut self.environment,
                CredentialSource::StoredKey => &mut self.stored_key,
            }),
            _ => None,
        }
    }

    fn advance(&mut self, settings: &Settings) {
        self.problem = None;
        match self.step {
            SetupStep::Connection => self.step = SetupStep::Model,
            SetupStep::Model => self.advance_from_model(),
            SetupStep::CredentialSource => self.step = SetupStep::Credential,
            SetupStep::Credential => {
                let valid = match self.credential_source {
                    CredentialSource::Environment => {
                        match valid_env_name(self.environment.text()) {
                            Ok(_) => true,
                            Err(error) => {
                                self.problem = Some(error);
                                false
                            }
                        }
                    }
                    CredentialSource::StoredKey => {
                        if self.stored_key.text().trim().is_empty() {
                            self.saved_key_available
                        } else if !settings.can_store_key() {
                            self.problem = Some(settings.storage_problem().to_owned());
                            false
                        } else {
                            self.stored_key_is_entered()
                        }
                    }
                };
                if valid {
                    self.review_selection = 0;
                    self.step = SetupStep::Review;
                } else if self.problem.is_none() {
                    self.problem = Some(if settings.can_store_key() {
                        "Enter a key, or use a saved key for this connection.".into()
                    } else {
                        settings.storage_problem().to_owned()
                    });
                }
            }
            SetupStep::Review => {}
        }
    }

    fn advance_from_model(&mut self) {
        match valid_model_id(self.model.text()) {
            Ok(model) => {
                self.model.set_text(model);
                self.step = SetupStep::CredentialSource;
                self.problem = None;
            }
            Err(error) => self.problem = Some(error),
        }
    }

    fn previous(&mut self) {
        self.problem = None;
        self.step = match self.step {
            SetupStep::Connection => return,
            SetupStep::Model => SetupStep::Connection,
            SetupStep::CredentialSource => SetupStep::Model,
            SetupStep::Credential => SetupStep::CredentialSource,
            SetupStep::Review => SetupStep::Credential,
        };
        if self.credential_source == CredentialSource::Environment {
            self.refresh_environment_detection();
        }
    }

    fn change_choice(&mut self, direction: isize, settings: &Settings) {
        let length = self.choices.len() as isize;
        self.selected = (self.selected as isize + direction).rem_euclid(length) as usize;
        self.load_selection(settings);
        self.problem = None;
    }

    fn load_selection(&mut self, settings: &Settings) {
        let choice = self.choice().clone();
        self.model.set_text(settings.model_for(&choice));
        self.environment.set_text(settings.environment_for(&choice));
        self.credential_source = settings.source_for(&choice);
        self.stored_key.set_text(String::new());
        self.saved_key_available = settings.saved_key_available(&choice);
        self.refresh_environment_detection();
    }

    fn refresh_environment_detection(&mut self) {
        self.environment_detected = self.environment_name().is_some_and(|name| {
            ApiKey::Env(name)
                .resolve()
                .is_ok_and(|value| !value.trim().is_empty())
        });
    }

    fn move_review(&mut self, direction: isize, mode: SetupMode) {
        let count = self.review_choices(mode).len() as isize;
        self.review_selection =
            (self.review_selection as isize + direction).rem_euclid(count) as usize;
    }

    pub(super) fn review_choices(&self, mode: SetupMode) -> Vec<ReviewAction> {
        let mut choices = Vec::new();
        if mode == SetupMode::FirstRun {
            if self.credential_ready() {
                choices.push(ReviewAction::SaveAndContinue);
            } else if self.can_store_key {
                choices.push(ReviewAction::RepairCredential);
            } else {
                choices.push(ReviewAction::Back);
            }
        }
        choices.push(ReviewAction::SaveAndExit);
        if !choices.contains(&ReviewAction::Back) {
            choices.push(ReviewAction::Back);
        }
        choices.push(ReviewAction::Cancel);
        choices
    }

    fn choose_review(&mut self, mode: SetupMode) {
        let choices = self.review_choices(mode);
        let Some(action) = choices.get(self.review_selection).copied() else {
            self.review_selection = 0;
            self.problem = Some("Choose a setup action.".into());
            return;
        };
        match action {
            ReviewAction::SaveAndContinue => self.finish = Some(Finish::SaveAndContinue),
            ReviewAction::SaveAndExit => self.finish = Some(Finish::SaveAndExit),
            ReviewAction::RepairCredential => {
                self.credential_source = CredentialSource::StoredKey;
                self.step = SetupStep::Credential;
                self.problem = None;
            }
            ReviewAction::Back => self.previous(),
            ReviewAction::Cancel => self.finish = Some(Finish::Cancel),
        }
    }
}

fn valid_model_id(input: &str) -> Result<String, String> {
    let model = input.trim();
    if model.is_empty() {
        return Err("Enter a model ID.".into());
    }
    if model.len() > MAX_MODEL_BYTES
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

pub(super) fn valid_env_name(input: &str) -> Result<String, String> {
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

pub(super) fn valid_stored_key(input: &str) -> Result<String, String> {
    if input.chars().any(char::is_control) {
        return Err("Keys must be a single line of at most 4096 bytes.".into());
    }
    let key = input.trim();
    if key.is_empty() {
        return Err("Enter a key or leave blank to keep the saved key.".into());
    }
    if key.len() > MAX_KEY_BYTES {
        return Err("Keys must be a single line of at most 4096 bytes.".into());
    }
    Ok(key.to_owned())
}
