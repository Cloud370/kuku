use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use kuku::adapter::{
    inspect_connection_model, inspect_model, model_list_targets, plan_model_trials,
    setup_api_candidates, setup_endpoint_format, setup_model_list_targets, DiscoveredModel,
    ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, TrialSelection,
};
use kuku::config::{
    resolve_request_url, ApiKey, ModelCapabilityDeclaration, ModelPricing, ProviderConfig,
    ProviderFormat, ProviderOptions, ProviderProxy, RequestedOutput, ThinkLevel, TokenRates,
};
use kuku::session::AgentConfig;
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

    pub(super) fn selected_in_catalog_order(&self) -> impl Iterator<Item = &str> {
        self.found
            .iter()
            .filter(|model| self.selected.contains(&model.id))
            .map(|model| model.id.as_str())
            .chain(self.selected.iter().filter_map(|id| {
                (!self.found.iter().any(|model| model.id == *id)).then_some(id.as_str())
            }))
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
        if self.full_url || self.recognized_request_url() {
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
        let format = setup_endpoint_format(self.address.text()).unwrap_or(self.protocol);
        resolve_request_url(format, &base, full.as_deref()).map_err(|error| error.to_string())
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
        options.proxy = self.proxy.clone();
        options.model_pricing = self.model_pricing.clone();
        let models_url = self.models_url.trim();
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

    pub(super) fn start_discovery(&mut self, settings: &Settings) {
        self.step = SetupStep::ModelSource;
        self.models.cursor = 0;
        if !self.authorized || !self.credential_ready() {
            self.problem = Some("Supply a credential to find models, or enter a model ID.".into());
            return;
        }
        let targets = self.connection(settings).and_then(|connection| {
            let infer_path = !self.address.text().trim().is_empty();
            if self.protocol_fixed {
                model_list_targets(&connection, infer_path)
            } else {
                setup_model_list_targets(&connection, infer_path)
            }
            .map_err(|error| error.to_string())
        });
        match targets {
            Ok(targets) => {
                self.problem = None;
                self.models.found.clear();
                self.models.discovered_from = None;
                self.discovery = ProbeBatch::start(targets.into_iter().map(Probe::from).collect());
                self.probe_repaint_at = Some(std::time::Instant::now());
            }
            Err(problem) => self.problem = Some(problem),
        }
    }

    fn accept_discovery(&mut self) {
        if !self.models.found.is_empty() {
            self.models.cursor = 0;
            self.step = SetupStep::Discover;
            return;
        }
        let Some((target, found)) = self.discovery.useful_catalog() else {
            return;
        };
        let connection = target.connection.clone();
        if self.address_parts().0 != connection.base_url && !self.full_url {
            self.address.set_text(connection.base_url.clone());
        }
        if !self.protocol_fixed {
            self.protocol = connection.format;
        }
        self.models.found = found.to_vec();
        self.models.discovered_from = Some(connection);
        self.models.cursor = 0;
        self.step = SetupStep::Discover;
    }

    pub(super) fn probe_deadline(&self) -> Option<std::time::Instant> {
        self.probe_repaint_at
            .filter(|_| self.discovery.pending() || self.trials.pending())
    }

    pub(super) fn poll_probes(&mut self, settings: &Settings, mode: super::SetupMode) -> bool {
        let now = std::time::Instant::now();
        if self.probe_deadline().is_none_or(|at| at > now) {
            return false;
        }
        let thinking_selection = (self.step == SetupStep::Thinking)
            .then(|| {
                self.thinking_choices(settings)
                    .get(self.models.cursor)
                    .cloned()
            })
            .flatten();
        let review_action = self.selected_review_action(mode);
        self.discovery.poll();
        self.trials.poll();
        if let Some(selected) = thinking_selection {
            self.models.cursor = self
                .thinking_choices(settings)
                .iter()
                .position(|choice| *choice == selected)
                .unwrap_or(0);
        }
        if self.step == SetupStep::ModelSource
            && self.models.cursor == 0
            && self.models.found.is_empty()
        {
            self.accept_discovery();
        }
        if self.step == SetupStep::CheckingApi {
            self.update_api_check(settings);
        }
        self.restore_review_action(mode, review_action);
        self.probe_repaint_at = Some(now + std::time::Duration::from_millis(100));
        true
    }

    pub(super) fn update_api_check(&mut self, settings: &Settings) {
        self.accept_api_check(settings);
        if self.step == SetupStep::CheckingApi {
            self.show_api_check_failure();
        }
    }

    pub(super) fn review_models(&mut self, settings: &Settings) {
        if matches!(
            self.step,
            SetupStep::Model | SetupStep::Discover | SetupStep::DefaultModel
        ) {
            self.model_review_from = self.step;
        }
        self.review_selection = 0;
        self.discovery.stop();
        if !self.protocol_fixed {
            if self.authorized && self.credential_ready() {
                self.start_api_check(settings);
            } else {
                self.step = SetupStep::CheckingApi;
                self.problem = Some("Supply an API key to check the API type.".into());
            }
            return;
        }
        let plan = if self.authorized && self.credential_ready() {
            self.planned_trials(settings)
        } else {
            Vec::new()
        };
        if plan.is_empty()
            || self
                .trials
                .rows()
                .iter()
                .take(plan.len())
                .map(|row| &row.probe)
                .ne(&plan)
        {
            self.trials = ProbeBatch::start(plan);
            self.probe_repaint_at = Some(std::time::Instant::now());
        }
        self.step = SetupStep::Review;
    }

    pub(super) fn start_api_check(&mut self, settings: &Settings) {
        self.step = SetupStep::CheckingApi;
        let Some(model) = self.primary_model() else {
            return;
        };
        let connection = match self.connection(settings) {
            Ok(connection) => connection,
            Err(problem) => {
                self.problem = Some(problem);
                self.trials = ProbeBatch::default();
                return;
            }
        };
        let candidates = setup_api_candidates(
            connection.format,
            self.models
                .found
                .iter()
                .filter(|found| self.models.selected.contains(&found.id))
                .map(|found| &found.capabilities),
        );
        if candidates.is_empty() {
            self.problem = Some("The selected models list different API types.".into());
            self.trials = ProbeBatch::default();
            return;
        }
        if let Some(row) = self.trials.rows().first() {
            if let (
                Probe::ApiType {
                    connection: before,
                    model: tested,
                    ..
                },
                ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))),
            ) = (&row.probe, &row.progress)
            {
                let mut confirmed = before.as_ref().clone();
                confirmed.format = evidence.format;
                confirmed.base_url.clone_from(&evidence.base_url);
                if *tested == model
                    && confirmed == connection
                    && candidates.contains(&evidence.format)
                {
                    let remaining = self.remaining_trials(settings);
                    if self
                        .trials
                        .rows()
                        .iter()
                        .skip(1)
                        .take(remaining.len())
                        .map(|row| &row.probe)
                        .eq(&remaining)
                    {
                        self.step = SetupStep::Review;
                        return;
                    }
                }
            }
        }
        self.trials = ProbeBatch::start(vec![Probe::ApiType {
            connection: Arc::new(connection),
            model,
            candidates,
        }]);
        self.probe_repaint_at = Some(std::time::Instant::now());
    }

    fn accept_api_check(&mut self, settings: &Settings) {
        let Some(row) = self.trials.rows().first() else {
            return;
        };
        let ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))) = &row.progress else {
            return;
        };
        self.protocol = evidence.format;
        if !self.full_url && self.address_parts().0 != evidence.base_url {
            self.address.set_text(evidence.base_url.clone());
        }
        if let Some(discovered) = &mut self.models.discovered_from {
            discovered.format = self.protocol;
        }
        self.trials.append(self.remaining_trials(settings));
        self.step = SetupStep::Review;
    }

    fn remaining_trials(&self, settings: &Settings) -> Vec<Probe> {
        let primary = self.primary_model();
        self.planned_trials(settings).into_iter().filter(|probe| {
            !matches!(probe, Probe::Trial { model, check: ModelTrial::Text { effort: None, service_tier: None }, .. } if Some(model) == primary.as_ref())
        }).collect()
    }

    fn planned_trials(&self, settings: &Settings) -> Vec<Probe> {
        let Ok(mut connection) = self.connection(settings) else {
            return Vec::new();
        };
        self.update_model_declarations(&mut connection);
        let original = self.route_name(settings).and_then(|name| {
            let route = settings.resolved.tier(name)?;
            let effort = inspect_model(
                &settings.resolved,
                &AgentConfig {
                    tier: Some(name.to_owned()),
                    ..AgentConfig::default()
                },
            )
            .ok()
            .and_then(|selection| selection.settings.reasoning_effort);
            Some((route, effort))
        });
        let primary = self.primary_model();
        let models = self.model_ids().into_iter().flatten().map(|model| {
            let existing = original.as_ref().filter(|(route, _)| {
                route.model == model
                    && matches!(self.choice(), ConnectionChoice::Existing(name) if &route.provider == name)
            });
            let effort = if self.reasoning_changed && primary.as_ref() == Some(&model) {
                self.reasoning.clone().filter(|effort| effort != ThinkLevel::ProviderDefault.as_str())
            } else {
                existing.and_then(|(_, effort)| effort.clone())
            };
            TrialSelection { model, effort, service_tier: existing.and_then(|(route, _)| route.service_tier.clone()) }
        });
        plan_model_trials(Arc::new(connection), models)
    }

    pub(super) fn trial_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.previous(),
            KeyCode::Up => self.models.cursor = self.models.cursor.saturating_sub(1),
            KeyCode::Down => {
                self.models.cursor = self
                    .models
                    .cursor
                    .saturating_add(1)
                    .min(self.trials.selected_rows().count().saturating_sub(1))
            }
            _ => {}
        }
    }

    pub(super) fn model_ids(&self) -> Result<Vec<String>, String> {
        if self.models.selected.is_empty() {
            Ok(vec![self.model_text()?])
        } else {
            Ok(self
                .models
                .selected_in_catalog_order()
                .map(str::to_owned)
                .collect())
        }
    }

    pub(super) fn primary_model(&self) -> Option<String> {
        self.models
            .default
            .as_ref()
            .filter(|id| self.models.selected.contains(*id))
            .cloned()
            .or_else(|| {
                self.models
                    .selected_in_catalog_order()
                    .next()
                    .map(str::to_owned)
            })
            .or_else(|| self.model_text().ok())
    }

    pub(super) fn update_capabilities(&self, connection: &mut ProviderConfig) {
        self.update_model_declarations(connection);
        for model in self.model_ids().into_iter().flatten() {
            let Some(test) = self.trials.thinking_test(connection, &model) else {
                continue;
            };
            let entry = connection
                .options
                .model_capabilities
                .entry(model)
                .or_default();
            if let Some(previous) = &mut entry.thinking_test {
                previous.merge(test);
            } else {
                entry.thinking_test = Some(test);
            }
        }
    }

    fn update_model_declarations(&self, connection: &mut ProviderConfig) {
        if self
            .models
            .discovered_from
            .as_ref()
            .is_some_and(|original| {
                original.format == connection.format
                    && original.base_url == connection.base_url
                    && original.api_key == connection.api_key
                    && original.options.endpoint_url == connection.options.endpoint_url
                    && original.options.models_url == connection.options.models_url
                    && original.options.proxy == connection.options.proxy
            })
        {
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
            .chain(self.capabilities(settings).into_iter().flat_map(|caps| {
                caps.selectable_reasoning_efforts()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            }))
            .collect()
    }

    pub(super) fn open_protocol_picker(&mut self, from_review: bool) {
        self.protocol_picker_from_review = from_review;
        self.models.cursor = PROTOCOLS
            .iter()
            .position(|format| *format == self.protocol)
            .expect("supported protocol");
        self.step = SetupStep::Protocol;
    }

    pub(super) fn choose_protocol(&mut self, key: KeyEvent, settings: &Settings) {
        if key.code == KeyCode::Enter {
            self.protocol = PROTOCOLS[self.models.cursor];
            self.protocol_fixed = true;
            if let Some(discovered) = &mut self.models.discovered_from {
                discovered.format = self.protocol;
            }
            self.problem = None;
            self.review_models(settings);
        } else {
            self.models.move_cursor(key.code, PROTOCOLS.len());
        }
    }

    pub(super) fn recognized_request_url(&self) -> bool {
        url::Url::parse(self.address.text().trim())
            .is_ok_and(|url| url.query().is_some() || setup_endpoint_format(url.as_str()).is_some())
    }

    pub(super) fn edit_address(&mut self, key: KeyEvent, settings: &Settings) {
        if key.code != KeyCode::Enter {
            let entered = self.address.text().to_owned();
            edit_field(&mut self.address, key);
            if self.address.text() != entered {
                self.full_url = false;
            }
            self.problem = None;
            return;
        }
        let entered = self.address.text().trim();
        if entered.is_empty() {
            if matches!(self.choice(), ConnectionChoice::Custom) {
                self.problem = Some("Enter the provider API URL.".into());
                return;
            }
            self.full_url = false;
        } else if let Ok(url) = url::Url::parse(entered) {
            if let Some(format) = setup_endpoint_format(entered) {
                self.protocol = format;
                self.protocol_fixed = true;
            } else if matches!(self.choice(), ConnectionChoice::Custom) {
                self.protocol_fixed = false;
            }
            self.full_url |= self.recognized_request_url();
            let path = url.path().trim_end_matches('/');
            let last = path.rsplit('/').next().unwrap_or_default();
            let versioned_base = last.strip_prefix('v').is_some_and(|suffix| {
                suffix.starts_with(|character: char| character.is_ascii_digit())
            });
            if !self.full_url && !path.is_empty() && !versioned_base && !entered.ends_with('/') {
                self.models.cursor = 0;
                self.step = SetupStep::AddressKind;
                return;
            }
        }
        self.finish_address(settings);
    }

    fn finish_address(&mut self, settings: &Settings) {
        match self.request_url() {
            Ok(_) => {
                self.problem = None;
                if self.apply_connection_repair(settings) {
                    return;
                }
                self.models.cursor = match self.proxy {
                    ProviderProxy::Environment => 0,
                    ProviderProxy::Direct => 1,
                    ProviderProxy::Explicit { .. } => 2,
                };
                self.step = SetupStep::Network;
            }
            Err(problem) => {
                self.problem = Some(problem);
                self.step = SetupStep::Address;
            }
        }
    }

    pub(super) fn choose_address_kind(&mut self, key: KeyEvent, settings: &Settings) {
        if key.code == KeyCode::Enter {
            self.full_url = self.models.cursor == 0;
            self.finish_address(settings);
        } else {
            self.models.move_cursor(key.code, 2);
        }
    }

    pub(super) fn choose_network(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            self.problem = None;
            self.step = match self.models.cursor {
                0 => {
                    self.proxy = ProviderProxy::Environment;
                    SetupStep::Credential
                }
                1 => {
                    self.proxy = ProviderProxy::Direct;
                    SetupStep::Credential
                }
                _ => SetupStep::ProxyAddress,
            };
        } else {
            self.models.move_cursor(key.code, 3);
        }
    }

    pub(super) fn edit_proxy(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            let url = self.proxy_address.text().trim().to_owned();
            let auth = match &self.proxy {
                ProviderProxy::Explicit { url: saved, auth } if *saved == url => auth.clone(),
                _ => None,
            };
            let proxy = ProviderProxy::Explicit { url, auth };
            match proxy.validate() {
                Ok(()) => {
                    self.proxy = proxy;
                    self.step = SetupStep::Credential;
                    self.problem = None;
                }
                Err(error) => self.problem = Some(error.to_string()),
            }
        } else {
            edit_field(&mut self.proxy_address, key);
            self.problem = None;
        }
    }

    pub(super) fn source_choices(&self) -> &'static [&'static str] {
        if self.discovery.useful_catalog().is_some() {
            &["Choose from list", "Enter model ID", "Try again"]
        } else if matches!(self.intent, SetupIntent::Edit(_)) && self.discovery.rows().is_empty() {
            &["Keep current model", "Enter model ID", "Find models"]
        } else if self.discovery.pending() {
            &["Waiting for model list", "Enter model ID"]
        } else {
            &["Find models", "Enter model ID"]
        }
    }

    pub(super) fn choose_source(&mut self, key: KeyEvent, settings: &Settings) {
        let choices = self.source_choices();
        if key.code != KeyCode::Enter {
            self.models.move_cursor(key.code, choices.len());
            return;
        }
        match choices.get(self.models.cursor).copied() {
            Some("Choose from list") => self.accept_discovery(),
            Some("Enter model ID") => {
                self.discovery.stop();
                self.problem = None;
                self.step = SetupStep::Model;
            }
            Some("Keep current model") => match self.model_text() {
                Ok(model) => {
                    self.models.add_manual(model);
                    self.trials = ProbeBatch::default();
                    self.model_review_from = SetupStep::ModelSource;
                    self.step = SetupStep::Review;
                    self.review_selection = 0;
                }
                Err(problem) => self.problem = Some(problem),
            },
            Some("Find models" | "Try again") => self.start_discovery(settings),
            _ => {}
        }
    }

    pub(super) fn choose_discovered(&mut self, key: KeyEvent, settings: &Settings) {
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                let count = self.models.filtered().count();
                self.models.move_cursor(key.code, count);
            }
            KeyCode::Char(' ') => {
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
            KeyCode::Enter => {
                if self.models.selected.is_empty() {
                    let model = self
                        .models
                        .filtered()
                        .nth(self.models.cursor)
                        .map(|item| item.id.clone());
                    if let Some(model) = model {
                        self.models.selected.insert(model);
                    }
                }
                self.advance_from_models(settings);
            }
            _ => {
                edit_field(&mut self.models.query, key);
                self.models.cursor = 0;
            }
        }
    }

    pub(super) fn advance_from_models(&mut self, settings: &Settings) {
        if self.models.selected.is_empty() {
            self.problem = Some("Choose a model, or enter a model ID manually.".into());
            return;
        }
        self.problem = None;
        self.models.cursor = 0;
        self.review_selection = 0;
        if self.intent == SetupIntent::Default && self.models.selected.len() > 1 {
            self.step = SetupStep::DefaultModel;
        } else {
            self.review_models(settings);
        }
    }

    pub(super) fn choose_default(&mut self, key: KeyEvent, settings: &Settings) {
        if key.code == KeyCode::Enter {
            let model = self
                .models
                .selected_in_catalog_order()
                .nth(self.models.cursor)
                .map(str::to_owned);
            self.models.default = model;
            self.models.cursor = 0;
            self.review_models(settings);
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
            self.step = SetupStep::ReviewSettings;
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
            self.step = SetupStep::ReviewSettings;
        } else {
            self.models.move_cursor(key.code, 2);
        }
    }

    pub(super) fn pricing_label(&self) -> &'static str {
        match self
            .primary_model()
            .and_then(|id| self.model_pricing.get(&id))
        {
            Some(ModelPricing::UsageOnly) => "Usage only",
            Some(ModelPricing::Configured { .. }) => "Configured USD rates",
            Some(ModelPricing::Automatic) | None => "Automatic",
        }
    }

    pub(super) fn choose_pricing(&mut self, key: KeyEvent) {
        if key.code != KeyCode::Enter {
            self.models.move_cursor(key.code, 3);
            return;
        }
        let Some(model) = self.primary_model() else {
            self.step = SetupStep::ReviewSettings;
            return;
        };
        if self.models.cursor != 1 {
            let mode = if self.models.cursor == 0 {
                ModelPricing::Automatic
            } else {
                ModelPricing::UsageOnly
            };
            self.model_pricing.insert(model, mode);
            self.step = SetupStep::ReviewSettings;
            return;
        }
        let rates = match self.model_pricing.get(&model) {
            Some(ModelPricing::Configured { rates }) => rates.clone(),
            _ => TokenRates::default(),
        };
        for (field, rate) in self.rate_fields.iter_mut().zip([
            rates.input,
            rates.output,
            rates.cache_read,
            rates.cache_write,
        ]) {
            field.set_text(rate.map_or_else(String::new, |rate| rate.to_string()));
        }
        self.models.cursor = 0;
        self.step = SetupStep::Rates;
    }

    pub(super) fn edit_rates(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Down => self.models.move_cursor(key.code, 5),
            KeyCode::Enter if self.models.cursor < 4 => self.models.cursor += 1,
            KeyCode::Enter => {
                let parse = |index: usize| {
                    let text = self.rate_fields[index].text().trim();
                    if text.is_empty() {
                        Ok(None)
                    } else {
                        text.parse::<f64>()
                            .ok()
                            .filter(|rate| rate.is_finite() && *rate >= 0.0)
                            .map(Some)
                            .ok_or(())
                    }
                };
                let (Ok(input), Ok(output), Ok(cache_read), Ok(cache_write)) =
                    (parse(0), parse(1), parse(2), parse(3))
                else {
                    self.problem =
                        Some("Enter nonnegative USD rates, or leave an unknown rate blank.".into());
                    return;
                };
                let Some(model) = self.primary_model() else {
                    return;
                };
                let original = match self.model_pricing.get(&model) {
                    Some(ModelPricing::Configured { rates }) => rates.clone(),
                    _ => TokenRates::default(),
                };
                self.model_pricing.insert(
                    model,
                    ModelPricing::Configured {
                        rates: TokenRates {
                            input,
                            output,
                            cache_read,
                            cache_write,
                            ..original
                        },
                    },
                );
                self.problem = None;
                self.step = SetupStep::ReviewSettings;
            }
            _ => {
                if let Some(field) = self.rate_fields.get_mut(self.models.cursor) {
                    edit_field(field, key);
                }
            }
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
