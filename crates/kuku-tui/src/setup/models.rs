use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::{KeyCode, KeyEvent};
use kuku::adapter::{inspect_connection_model, DiscoveredModel};
use kuku::config::{
    resolve_models_url, resolve_request_url, ApiKey, ModelCapabilityDeclaration, ProviderConfig,
    ProviderFormat, ProviderOptions, RequestedOutput, ThinkLevel,
};
use kuku::ProviderCapabilities;

use crate::editor::Editor;

use super::config::Settings;
use super::state::{edit_field, SetupIntent, SetupState, SetupStep};
use super::{ConnectionChoice, CredentialSource};

pub(super) const PROTOCOLS: [ProviderFormat; 3] = [
    ProviderFormat::OpenAiChat,
    ProviderFormat::OpenAiResponses,
    ProviderFormat::Anthropic,
];

pub(super) fn protocol_label(protocol: ProviderFormat) -> &'static str {
    match protocol {
        ProviderFormat::OpenAiChat => "OpenAI Chat Completions",
        ProviderFormat::OpenAiResponses => "OpenAI Responses",
        ProviderFormat::Anthropic => "Anthropic Messages",
    }
}

#[derive(Default)]
pub(super) struct Models {
    pub(super) cursor: usize,
    pub(super) query: Editor,
    pub(super) found: Vec<DiscoveredModel>,
    pub(super) selected: BTreeSet<String>,
    pub(super) manual: Option<String>,
    pub(super) default: Option<String>,
    pub(super) overrides: BTreeMap<String, ModelCapabilityDeclaration>,
    pub(super) override_value: Editor,
    pub(super) discovered_from: Option<ProviderConfig>,
}

impl Models {
    pub(super) fn add_manual(&mut self, model: String) {
        if let Some(previous) = self.manual.replace(model.clone()) {
            self.selected.remove(&previous);
        }
        self.selected.insert(model);
    }

    pub(super) fn filtered(&self) -> impl Iterator<Item = &DiscoveredModel> {
        let query = self.query.text().trim().to_lowercase();
        self.found.iter().filter(move |model| {
            model.id.to_lowercase().contains(&query)
                || model
                    .name
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase().contains(&query))
        })
    }

    pub(super) fn move_cursor(&mut self, code: KeyCode, count: usize) {
        let direction = match code {
            KeyCode::Up | KeyCode::Left => -1,
            KeyCode::Down | KeyCode::Right => 1,
            _ => return,
        };
        if count > 0 {
            self.cursor = (self.cursor as isize + direction).rem_euclid(count as isize) as usize;
        }
    }
}

impl SetupState {
    pub(super) fn route_name<'a>(&'a self, settings: &'a Settings) -> Option<&'a str> {
        match &self.intent {
            SetupIntent::Edit(name) => Some(name),
            SetupIntent::Default if settings.existing => Some(&settings.default_tier),
            SetupIntent::Default | SetupIntent::Add => None,
        }
    }

    pub(super) fn default_base_url(&self) -> &'static str {
        match self.choice() {
            ConnectionChoice::Preset(preset) => preset.base_url(self.protocol),
            ConnectionChoice::Existing(_) | ConnectionChoice::Custom => {
                self.protocol.default_base_url()
            }
        }
    }

    pub(super) fn address_parts(&self) -> (String, Option<String>) {
        let entered = self.address.text().trim();
        if self.full_url {
            (String::new(), Some(entered.to_owned()))
        } else {
            let base = if entered.is_empty() {
                self.default_base_url()
            } else {
                entered
            };
            (base.to_owned(), None)
        }
    }

    pub(super) fn request_url(&self) -> Result<String, String> {
        let (base, full) = self.address_parts();
        resolve_request_url(self.protocol, &base, full.as_deref())
            .map_err(|error| error.to_string())
    }

    pub(super) fn connection_options(&self, settings: &Settings) -> ProviderOptions {
        let mut options = match self.choice() {
            ConnectionChoice::Existing(name) => settings.resolved.providers[name].options.clone(),
            ConnectionChoice::Preset(preset) => ProviderOptions {
                profile: preset.profile(),
                ..ProviderOptions::default()
            },
            ConnectionChoice::Custom => ProviderOptions::default(),
        };
        options.endpoint_url = self.address_parts().1;
        let models_url = self.models_url.text().trim();
        options.models_url = (!models_url.is_empty()).then(|| models_url.to_owned());
        options
    }

    pub(super) fn connection(&self, settings: &Settings) -> Result<ProviderConfig, String> {
        let api_key = match self.credential_source {
            CredentialSource::Environment => {
                ApiKey::Env(super::state::valid_env_name(self.environment.text())?)
            }
            CredentialSource::StoredKey if self.stored_key.text().trim().is_empty() => settings
                .key_for(self.choice())
                .cloned()
                .ok_or_else(|| "Enter a key or choose an environment variable.".to_owned())?,
            CredentialSource::StoredKey => {
                ApiKey::Plaintext(super::state::valid_stored_key(self.stored_key.text())?)
            }
        };
        Ok(ProviderConfig {
            format: self.protocol,
            base_url: self.address_parts().0,
            api_key,
            options: self.connection_options(settings),
        })
    }

    pub(super) fn take_discovery(&mut self, settings: &Settings) -> Option<ProviderConfig> {
        if !std::mem::take(&mut self.discover_now) {
            return None;
        }
        match self.connection(settings) {
            Ok(connection) => {
                self.discovery_pending = true;
                if self.models.discovered_from.as_ref() != Some(&connection) {
                    self.models.found.clear();
                }
                self.models.discovered_from = Some(connection.clone());
                Some(connection)
            }
            Err(problem) => {
                self.problem = Some(problem);
                None
            }
        }
    }

    pub(super) fn discovery_finished(&mut self, result: Result<Vec<DiscoveredModel>, String>) {
        self.discovery_pending = false;
        match result {
            Ok(models) => {
                self.models.found = models;
                self.models.cursor = 0;
                if self.models.found.is_empty() {
                    self.problem =
                        Some("The provider returned no models. Enter a model ID manually.".into());
                    self.step = SetupStep::ModelSource;
                } else {
                    self.step = SetupStep::Discover;
                }
            }
            Err(problem) => {
                self.problem = Some(problem);
                self.step = SetupStep::ModelSource;
            }
        }
    }

    pub(super) fn model_ids(&self) -> Result<Vec<String>, String> {
        if self.models.selected.is_empty() {
            Ok(vec![self.model_text()?])
        } else {
            Ok(self.models.selected.iter().cloned().collect())
        }
    }

    pub(super) fn primary_model(&self) -> Option<String> {
        self.models
            .default
            .as_ref()
            .filter(|id| self.models.selected.contains(*id))
            .cloned()
            .or_else(|| self.models.selected.iter().next().cloned())
            .or_else(|| self.model_text().ok())
    }

    pub(super) fn update_capabilities(&self, connection: &mut ProviderConfig) {
        if self.models.discovered_from.as_ref() == Some(connection) {
            for model in &self.models.found {
                if !self.models.selected.contains(&model.id)
                    && !connection
                        .options
                        .model_capabilities
                        .contains_key(&model.id)
                    && self.model.text() != model.id
                {
                    continue;
                }
                connection
                    .options
                    .model_capabilities
                    .entry(model.id.clone())
                    .or_default()
                    .discovered = Some(model.capabilities.clone());
            }
        }
        for (id, manual) in &self.models.overrides {
            connection
                .options
                .model_capabilities
                .entry(id.clone())
                .or_default()
                .manual = Some(manual.clone());
        }
    }

    pub(super) fn capabilities(&self, settings: &Settings) -> Option<ProviderCapabilities> {
        let mut connection = self.connection(settings).ok()?;
        self.update_capabilities(&mut connection);
        inspect_connection_model(&connection, &self.primary_model()?).ok()
    }

    pub(super) fn thinking_choices(&self, settings: &Settings) -> Vec<String> {
        std::iter::once(ThinkLevel::ProviderDefault.as_str().to_owned())
            .chain(
                self.capabilities(settings)
                    .into_iter()
                    .flat_map(|caps| caps.reasoning_efforts),
            )
            .collect()
    }

    pub(super) fn choose_protocol(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            self.step = SetupStep::Address;
        } else {
            let index = PROTOCOLS
                .iter()
                .position(|format| *format == self.protocol)
                .expect("supported protocol");
            self.models.cursor = index;
            self.models.move_cursor(key.code, PROTOCOLS.len());
            self.protocol = PROTOCOLS[self.models.cursor];
        }
    }

    pub(super) fn edit_address(&mut self, key: KeyEvent, settings: &Settings) {
        match key.code {
            KeyCode::F(2) => self.full_url = !self.full_url,
            KeyCode::Enter | KeyCode::Tab => match self.request_url() {
                Ok(_) => {
                    self.problem = None;
                    self.step = SetupStep::ModelsUrl;
                }
                Err(problem) => self.problem = Some(problem),
            },
            _ => edit_field(&mut self.address, key),
        }
        if key.code == KeyCode::F(2) && self.full_url && self.address.text().trim().is_empty() {
            let base = self.default_base_url();
            if let Ok(preview) = resolve_request_url(self.protocol, base, None) {
                self.address.set_text(preview);
            }
        }
        if self.step == SetupStep::ModelsUrl
            && !self.full_url
            && self
                .address
                .text()
                .trim_end_matches('/')
                .ends_with(self.protocol.request_suffix())
        {
            self.problem =
                Some("That address looks like a full endpoint. Press F2 to use it exactly.".into());
            self.step = SetupStep::Address;
        }
        if self.step == SetupStep::Address
            && key.code == KeyCode::F(2)
            && matches!(self.choice(), ConnectionChoice::Existing(_))
        {
            self.problem = Some(format!(
                "Review the shared connection before saving · {}",
                settings.choice_label(self.choice())
            ));
        }
    }

    pub(super) fn edit_models_url(&mut self, key: KeyEvent, settings: &Settings) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            let (base, _) = self.address_parts();
            let options = self.connection_options(settings);
            if options.models_url.is_some() {
                if let Err(error) = resolve_models_url(self.protocol, &base, &options) {
                    self.problem = Some(error.to_string());
                    return;
                }
            }
            self.problem = None;
            self.step = SetupStep::CredentialSource;
        } else {
            edit_field(&mut self.models_url, key);
        }
    }

    pub(super) fn choose_source(&mut self, key: KeyEvent, settings: &Settings) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            if self.models.cursor == 0 {
                self.problem = None;
                self.step = SetupStep::Model;
            } else if self.models.found.is_empty()
                || self.models.discovered_from.as_ref() != self.connection(settings).ok().as_ref()
            {
                self.discover_now = true;
                self.problem = None;
            } else {
                self.step = SetupStep::Discover;
                self.models.cursor = 0;
            }
        } else if key.code == KeyCode::F(5) && self.models.cursor == 1 {
            self.discover_now = true;
        } else {
            self.models.cursor = self.models.cursor.min(1);
            self.models.move_cursor(key.code, 2);
        }
    }

    pub(super) fn choose_discovered(&mut self, key: KeyEvent, _settings: &Settings) {
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                let count = self.models.filtered().count();
                self.models.move_cursor(key.code, count);
            }
            KeyCode::F(5) => self.discover_now = true,
            KeyCode::Char(' ') | KeyCode::Enter => {
                let model = self
                    .models
                    .filtered()
                    .nth(self.models.cursor)
                    .map(|item| item.id.clone());
                if let Some(model) = model {
                    if self.models.selected.contains(&model) {
                        self.models.selected.remove(&model);
                    } else {
                        if matches!(self.intent, SetupIntent::Edit(_)) {
                            self.models.selected.clear();
                        }
                        self.models.selected.insert(model);
                    }
                }
            }
            KeyCode::Tab => self.advance_from_models(),
            _ => {
                edit_field(&mut self.models.query, key);
                self.models.cursor = 0;
            }
        }
    }

    pub(super) fn advance_from_models(&mut self) {
        if self.models.selected.is_empty() {
            self.problem = Some("Select at least one model, or enter a model ID manually.".into());
            return;
        }
        self.problem = None;
        self.models.cursor = 0;
        self.review_selection = 0;
        self.step = if self.intent == SetupIntent::Default && self.models.selected.len() > 1 {
            SetupStep::DefaultModel
        } else {
            SetupStep::Review
        };
    }

    pub(super) fn choose_default(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            self.models.default = self.models.selected.iter().nth(self.models.cursor).cloned();
            self.step = SetupStep::Review;
        } else {
            self.models
                .move_cursor(key.code, self.models.selected.len());
        }
    }

    pub(super) fn choose_thinking(&mut self, key: KeyEvent, settings: &Settings) {
        let choices = self.thinking_choices(settings);
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            self.reasoning = choices.get(self.models.cursor).cloned();
            self.reasoning_changed = true;
            self.step = SetupStep::Review;
        } else {
            self.models.move_cursor(key.code, choices.len());
        }
    }

    pub(super) fn choose_output(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            self.output = if self.models.cursor == 0 {
                RequestedOutput::Text
            } else {
                RequestedOutput::Image
            };
            self.output_changed = true;
            self.step = SetupStep::Review;
        } else {
            self.models.move_cursor(key.code, 2);
        }
    }

    pub(super) fn choose_advanced(&mut self, key: KeyEvent, settings: &Settings) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            let Some(id) = self.primary_model() else {
                return;
            };
            let saved = self
                .connection_options(settings)
                .model_capabilities
                .remove(&id)
                .and_then(|entry| entry.manual);
            let declaration = self
                .models
                .overrides
                .entry(id)
                .or_insert_with(|| saved.unwrap_or_default());
            let value = match self.models.cursor {
                0 => declaration
                    .reasoning_efforts
                    .as_ref()
                    .map(|items| items.join(", ")),
                1 => declaration.reasoning_default.clone(),
                2 => declaration
                    .input_modalities
                    .as_ref()
                    .map(|items| items.join(", ")),
                3 => declaration
                    .output_modalities
                    .as_ref()
                    .map(|items| items.join(", ")),
                _ => declaration
                    .interfaces
                    .as_ref()
                    .map(|items| items.join(", ")),
            };
            self.models
                .override_value
                .set_text(value.unwrap_or_default());
            self.step = SetupStep::Override;
        } else {
            self.models.move_cursor(key.code, 5);
        }
    }

    pub(super) fn edit_override(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            let Some(id) = self.primary_model() else {
                return;
            };
            let declaration = self.models.overrides.entry(id).or_default();
            let text = self.models.override_value.text().trim();
            let list = (!text.is_empty()).then(|| {
                text.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect()
            });
            match self.models.cursor {
                0 => declaration.reasoning_efforts = list,
                1 => declaration.reasoning_default = (!text.is_empty()).then(|| text.to_owned()),
                2 => declaration.input_modalities = list,
                3 => declaration.output_modalities = list,
                _ => declaration.interfaces = list,
            }
            self.step = SetupStep::Advanced;
        } else {
            edit_field(&mut self.models.override_value, key);
        }
    }
}
