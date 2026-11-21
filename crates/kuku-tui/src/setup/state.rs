use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kuku::adapter::ProbeBatch;
use kuku::config::{ApiKey, ModelPricing, ProviderFormat, ProviderProxy, RequestedOutput};

use crate::editor::Editor;

use super::config::Settings;
use super::models::Models;
use super::{ConnectionChoice, CredentialSource, Finish, SetupMode};

const MAX_MODEL_BYTES: usize = 256;
const MAX_KEY_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetupStep {
    Consent,
    ManualConfig,
    Connection,
    Protocol,
    Address,
    AddressKind,
    Network,
    ProxyAddress,
    ModelSource,
    Discover,
    TrialPlan,
    Trials,
    DefaultModel,
    Model,
    Thinking,
    Output,
    Pricing,
    Rates,
    Advanced,
    Override,
    CredentialSource,
    Credential,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewAction {
    SaveAndContinue,
    SaveAndExit,
    Save,
    Thinking,
    Output,
    Advanced,
    RepairCredential,
    TrialResults,
    Pricing,
    Back,
    Cancel,
}

impl ReviewAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SaveAndContinue => "Save and continue",
            Self::SaveAndExit => "Save and exit",
            Self::Save => "Save models",
            Self::Thinking => "Thinking",
            Self::Output => "Requested output",
            Self::Advanced => "Advanced model capabilities",
            Self::RepairCredential => "Paste API key",
            Self::TrialResults => "Model trial and results",
            Self::Pricing => "Cost and rates",
            Self::Back => "Back",
            Self::Cancel => "Cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupIntent {
    Default,
    Add,
    Edit(String),
}

pub(super) struct SetupState {
    pub(super) choices: Vec<ConnectionChoice>,
    pub(super) selected: usize,
    pub(super) intent: SetupIntent,
    pub(super) protocol: ProviderFormat,
    pub(super) full_url: bool,
    pub(super) address: Editor,
    pub(super) models_url: String,
    pub(super) proxy: ProviderProxy,
    pub(super) proxy_address: Editor,
    pub(super) model_pricing: std::collections::BTreeMap<String, ModelPricing>,
    pub(super) rate_fields: [Editor; 4],
    pub(super) authorized: bool,
    pub(super) discovery: ProbeBatch,
    pub(super) trials: ProbeBatch,
    pub(super) probe_repaint_at: Option<std::time::Instant>,
    pub(super) models: Models,
    pub(super) reasoning: Option<String>,
    pub(super) output: RequestedOutput,
    pub(super) reasoning_changed: bool,
    pub(super) output_changed: bool,
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
        Self::for_intent(settings, SetupIntent::Default)
    }

    pub(super) fn for_intent(settings: &Settings, intent: SetupIntent) -> Self {
        let edit = match &intent {
            SetupIntent::Edit(tier) => settings.resolved.tier(tier),
            SetupIntent::Default | SetupIntent::Add => None,
        };
        let initial = edit.map_or_else(
            || settings.initial_choice(),
            |tier| ConnectionChoice::Existing(tier.provider.clone()),
        );
        let choices = settings.connection_choices();
        let selected = choices
            .iter()
            .position(|choice| *choice == initial)
            .unwrap_or(0);
        let mut state = Self {
            choices,
            selected,
            step: SetupStep::Consent,
            intent,
            protocol: ProviderFormat::OpenAiChat,
            full_url: false,
            address: Editor::default(),
            models_url: String::new(),
            proxy: ProviderProxy::default(),
            proxy_address: Editor::default(),
            model_pricing: Default::default(),
            rate_fields: Default::default(),
            authorized: false,
            discovery: ProbeBatch::default(),
            trials: ProbeBatch::default(),
            probe_repaint_at: None,
            models: Models::default(),
            reasoning: None,
            output: RequestedOutput::Text,
            reasoning_changed: false,
            output_changed: false,
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
                let Some(editor) = self.editor_mut() else {
                    return;
                };
                editor.insert(&text);
                if self.step == SetupStep::Discover {
                    self.models.cursor = 0;
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
            if self.step == SetupStep::Consent {
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
            SetupStep::Consent => match key.code {
                KeyCode::Up | KeyCode::Down => self.models.move_cursor(key.code, 2),
                KeyCode::Enter => {
                    if self.models.cursor == 0 {
                        self.authorized = true;
                        self.step = if matches!(self.intent, SetupIntent::Edit(_)) {
                            SetupStep::Protocol
                        } else {
                            SetupStep::Connection
                        };
                    } else {
                        self.step = SetupStep::ManualConfig;
                    }
                }
                _ => {}
            },
            SetupStep::ManualConfig => {
                if key.code == KeyCode::Enter {
                    self.finish = Some(Finish::Cancel);
                }
            }
            SetupStep::Connection => match key.code {
                KeyCode::Up | KeyCode::Left => self.change_choice(-1, settings),
                KeyCode::Down | KeyCode::Right => self.change_choice(1, settings),
                KeyCode::Tab | KeyCode::Enter => self.advance(settings),
                _ => {}
            },
            SetupStep::Protocol => self.choose_protocol(key),
            SetupStep::Address => self.edit_address(key),
            SetupStep::AddressKind => self.choose_address_kind(key),
            SetupStep::Network => self.choose_network(key),
            SetupStep::ProxyAddress => self.edit_proxy(key),
            SetupStep::ModelSource => self.choose_source(key, settings),
            SetupStep::Discover => self.choose_discovered(key),
            SetupStep::TrialPlan => self.choose_trial_plan(key, settings),
            SetupStep::Trials => self.trial_input(key),
            SetupStep::DefaultModel => self.choose_default(key),
            SetupStep::Model => self.edit_model(key),
            SetupStep::Thinking => self.choose_thinking(key, settings),
            SetupStep::Output => self.choose_output(key),
            SetupStep::Pricing => self.choose_pricing(key),
            SetupStep::Rates => self.edit_rates(key),
            SetupStep::Advanced => self.choose_advanced(key, settings),
            SetupStep::Override => self.edit_override(key),
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
        if matches!(key.code, KeyCode::Tab | KeyCode::Enter) {
            self.advance_from_model();
        } else {
            edit_field(&mut self.model, key);
            self.problem = None;
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
            SetupStep::Address => Some(&mut self.address),
            SetupStep::ProxyAddress => Some(&mut self.proxy_address),
            SetupStep::Discover => Some(&mut self.models.query),
            SetupStep::Override => Some(&mut self.models.override_value),
            SetupStep::Rates => self.rate_fields.get_mut(self.models.cursor),
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
            SetupStep::Connection => {
                self.models.cursor = 0;
                if self.intent == SetupIntent::Add
                    && matches!(self.choice(), ConnectionChoice::Existing(_))
                {
                    self.start_discovery(settings);
                } else {
                    self.step = SetupStep::Protocol;
                }
            }
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
                    self.models.cursor = 0;
                    if matches!(self.intent, SetupIntent::Edit(_)) {
                        self.step = SetupStep::ModelSource;
                    } else {
                        self.start_discovery(settings);
                    }
                } else if self.problem.is_none() {
                    self.problem = Some(if settings.can_store_key() {
                        "Enter a key, or use a saved key for this connection.".into()
                    } else {
                        settings.storage_problem().to_owned()
                    });
                }
            }
            SetupStep::Consent
            | SetupStep::ManualConfig
            | SetupStep::Protocol
            | SetupStep::Address
            | SetupStep::AddressKind
            | SetupStep::Network
            | SetupStep::ProxyAddress
            | SetupStep::ModelSource
            | SetupStep::Discover
            | SetupStep::TrialPlan
            | SetupStep::Trials
            | SetupStep::DefaultModel
            | SetupStep::Thinking
            | SetupStep::Output
            | SetupStep::Pricing
            | SetupStep::Rates
            | SetupStep::Advanced
            | SetupStep::Override
            | SetupStep::Review => {}
        }
    }

    fn advance_from_model(&mut self) {
        match valid_model_id(self.model.text()) {
            Ok(model) => {
                self.model.set_text(model.clone());
                self.models.add_manual(model);
                self.advance_from_models();
            }
            Err(error) => self.problem = Some(error),
        }
    }

    pub(super) fn previous(&mut self) {
        self.problem = None;
        self.models.cursor = 0;
        self.step = match self.step {
            SetupStep::Consent => return,
            SetupStep::ManualConfig | SetupStep::Connection => SetupStep::Consent,
            SetupStep::Protocol => SetupStep::Connection,
            SetupStep::Address => SetupStep::Protocol,
            SetupStep::AddressKind | SetupStep::Network => SetupStep::Address,
            SetupStep::ProxyAddress => SetupStep::Network,
            SetupStep::CredentialSource => SetupStep::Network,
            SetupStep::Credential => SetupStep::CredentialSource,
            SetupStep::ModelSource => {
                self.discovery.stop();
                if self.intent == SetupIntent::Add
                    && matches!(self.choice(), ConnectionChoice::Existing(_))
                {
                    SetupStep::Connection
                } else {
                    SetupStep::Credential
                }
            }
            SetupStep::Trials => {
                self.trials.stop();
                SetupStep::Review
            }
            SetupStep::TrialPlan => SetupStep::ModelSource,
            SetupStep::Model | SetupStep::Discover => SetupStep::ModelSource,
            SetupStep::DefaultModel => SetupStep::ModelSource,
            SetupStep::Review => SetupStep::ModelSource,
            SetupStep::Thinking | SetupStep::Output | SetupStep::Advanced | SetupStep::Pricing => {
                SetupStep::Review
            }
            SetupStep::Rates => SetupStep::Pricing,
            SetupStep::Override => SetupStep::Advanced,
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

    pub(super) fn load_selection(&mut self, settings: &Settings) {
        let choice = self.choice().clone();
        let original = self
            .route_name(settings)
            .and_then(|name| settings.resolved.tier(name));
        let initial_model = original.filter(
            |tier| matches!(&choice, ConnectionChoice::Existing(name) if *name == tier.provider),
        );
        self.protocol = settings.protocol_for(&choice);
        let (address, full_url, models_url) = settings.address_for(&choice);
        self.address.set_text(address);
        self.full_url = full_url;
        self.models_url = models_url;
        self.proxy = match &choice {
            ConnectionChoice::Existing(name) => {
                settings.resolved.providers[name].options.proxy.clone()
            }
            ConnectionChoice::Preset(_) | ConnectionChoice::Custom => ProviderProxy::Environment,
        };
        self.model_pricing = match &choice {
            ConnectionChoice::Existing(name) => settings.resolved.providers[name]
                .options
                .model_pricing
                .clone(),
            _ => Default::default(),
        };
        self.proxy_address.set_text(match &self.proxy {
            ProviderProxy::Explicit { url, .. } => url.clone(),
            _ => String::new(),
        });
        self.discovery.stop();
        self.trials.stop();
        self.discovery = ProbeBatch::default();
        self.trials = ProbeBatch::default();
        self.models = Models::default();
        self.model.set_text(match &self.intent {
            SetupIntent::Add if matches!(choice, ConnectionChoice::Existing(_)) => String::new(),
            _ => {
                initial_model.map_or_else(|| settings.model_for(&choice), |tier| tier.model.clone())
            }
        });
        self.reasoning = initial_model.map(|tier| {
            tier.reasoning_effort
                .clone()
                .unwrap_or_else(|| tier.think.as_str().into())
        });
        self.output = initial_model.map_or(RequestedOutput::Text, |tier| tier.output);
        self.reasoning_changed = false;
        self.output_changed = false;
        self.environment.set_text(settings.environment_for(&choice));
        self.credential_source = if self.intent == SetupIntent::Add
            && matches!(settings.key_for(&choice), Some(ApiKey::Env(_)))
        {
            CredentialSource::Environment
        } else {
            settings.source_for(&choice)
        };
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
        choices.push(if mode == SetupMode::Session {
            ReviewAction::Save
        } else {
            ReviewAction::SaveAndExit
        });
        if !choices.contains(&ReviewAction::Back) {
            choices.push(ReviewAction::Back);
        }
        choices.push(ReviewAction::Cancel);
        choices.extend([
            ReviewAction::Thinking,
            ReviewAction::Output,
            ReviewAction::Advanced,
            ReviewAction::TrialResults,
            ReviewAction::Pricing,
        ]);
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
            ReviewAction::Save => self.finish = Some(Finish::SaveAndContinue),
            ReviewAction::Thinking => {
                self.models.cursor = 0;
                self.step = SetupStep::Thinking;
            }
            ReviewAction::Output => {
                self.models.cursor = usize::from(self.output == RequestedOutput::Image);
                self.step = SetupStep::Output;
            }
            ReviewAction::Advanced => {
                self.models.cursor = 0;
                self.step = SetupStep::Advanced;
            }
            ReviewAction::RepairCredential => {
                self.credential_source = CredentialSource::StoredKey;
                self.step = SetupStep::Credential;
                self.problem = None;
            }
            ReviewAction::TrialResults => {
                if self.trials.rows().is_empty() {
                    self.offer_trials();
                } else {
                    self.step = SetupStep::Trials;
                }
            }
            ReviewAction::Pricing => {
                self.models.cursor = self
                    .primary_model()
                    .as_deref()
                    .and_then(|model| self.model_pricing.get(model))
                    .map_or(0, |pricing| match pricing {
                        ModelPricing::Automatic => 0,
                        ModelPricing::Configured { .. } => 1,
                        ModelPricing::UsageOnly => 2,
                    });
                self.step = SetupStep::Pricing;
            }
            ReviewAction::Back => self.previous(),
            ReviewAction::Cancel => self.finish = Some(Finish::Cancel),
        }
    }
}

pub(super) fn valid_model_id(input: &str) -> Result<String, String> {
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

pub(super) fn edit_field(editor: &mut Editor, key: KeyEvent) {
    match key.code {
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete(),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            editor.insert(&character.to_string());
        }
        _ => {}
    }
}
