use std::time::Duration;

use crossterm::event::Event;
use kuku::adapter::inspect_model;
use kuku::config::ThinkLevel;
use kuku::session::AgentConfig;
use kuku_client::local::ServiceIdentity;
use kuku_client::{Connection, ConnectionOptions};
use ratatui::Frame;

use crate::local_settings::{active_service, ensure_same_service};
use crate::model_default::ResolvedModel;

use super::config::{self, Settings};
use super::state::{SetupIntent, SetupState};
use super::{Finish, SetupMode};

pub(crate) struct ModelEditor {
    settings: Settings,
    state: SetupState,
    service: ServiceIdentity,
}

pub(crate) enum ModelEditorAction {
    None,
    Cancel,
    Save,
}

#[derive(Debug, Clone)]
pub(crate) enum SavedModelSetting {
    Default(String),
    Thinking {
        target: ThinkingSaveTarget,
        effort: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ThinkingSaveTarget {
    route: ResolvedModel,
    tier: String,
    requested_tier: Option<String>,
    service: ServiceIdentity,
}

pub(crate) struct ThinkingDefaults {
    pub(crate) inherited: Option<kuku::ProviderSettings>,
    pub(crate) save: Option<ThinkingSaveTarget>,
}

impl ModelEditor {
    pub(crate) async fn open(
        options: &ConnectionOptions,
        connection: Option<&Connection>,
        intent: SetupIntent,
    ) -> Result<Self, String> {
        let (service, settings) = active_settings(options, connection).await?;
        if matches!(&intent, SetupIntent::Edit(route) if settings.resolved.tier(route).is_none()) {
            return Err("The model route changed. Reopen /model.".into());
        }
        let state = SetupState::for_intent(&settings, intent);
        Ok(Self {
            settings,
            state,
            service,
        })
    }

    pub(crate) fn handle(&mut self, event: Event) -> ModelEditorAction {
        self.state.handle(event, &self.settings, SetupMode::Session);
        match self.state.finish.take() {
            Some(Finish::Cancel) => ModelEditorAction::Cancel,
            Some(Finish::SaveAndContinue | Finish::SaveAndExit) => ModelEditorAction::Save,
            None => ModelEditorAction::None,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>) {
        super::ui::render(frame, &self.settings, &mut self.state, SetupMode::Session);
    }

    pub(crate) fn probe_deadline(&self) -> Option<std::time::Instant> {
        self.state.probe_deadline()
    }

    pub(crate) fn poll_probes(&mut self) -> bool {
        self.state.poll_probes(&self.settings, SetupMode::Session)
    }

    pub(crate) fn failed(&mut self, problem: String) {
        self.state.problem = Some(problem);
    }

    pub(crate) async fn save(
        &self,
        options: &ConnectionOptions,
        connection: Option<&Connection>,
    ) -> Result<bool, String> {
        let current = active_service(options, connection).await?;
        ensure_same_service(&self.service, &current)?;
        let config =
            config::save(&self.settings, &self.state).map_err(|error| error.to_string())?;
        Ok(wait_for_catalog(connection, &config).await)
    }
}

pub(crate) async fn thinking_defaults(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    mut config: AgentConfig,
    route: ResolvedModel,
) -> ThinkingDefaults {
    let tier = config.tier.clone();
    config.reasoning_effort = None;
    let inherited = async {
        let resolved = connection?.client.resolve_model(&config).await.ok()?;
        (resolved.connection == route.connection && resolved.model == route.model)
            .then_some(resolved.settings)
    };
    let save = async {
        let (service, settings) = active_settings(options, connection).await.ok()?;
        let name = thinking_tier(&settings, tier.as_deref(), &route)?.to_owned();
        Some(ThinkingSaveTarget {
            route: route.clone(),
            tier: name,
            requested_tier: tier,
            service,
        })
    };
    let (inherited, save) = tokio::join!(inherited, save);
    ThinkingDefaults { inherited, save }
}

pub(crate) async fn save_setting(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    change: SavedModelSetting,
) -> Result<bool, String> {
    let (service, settings) = active_settings(options, connection).await?;
    let current = active_service(options, connection).await?;
    ensure_same_service(&service, &current)?;
    let (route, effort) = match &change {
        SavedModelSetting::Default(route) => (route.as_str(), None),
        SavedModelSetting::Thinking { target, effort } => {
            ensure_same_service(&target.service, &current)?;
            let route = thinking_tier(&settings, target.requested_tier.as_deref(), &target.route)
                .filter(|tier| *tier == target.tier)
                .ok_or_else(|| {
                    "Choose a configured model route in /model before saving its thinking setting."
                        .to_owned()
                })?;
            (route, Some(effort.as_str()))
        }
    };
    let mut document = settings.document.clone();
    if let Some(effort) = effort {
        let fields =
            config::entry(&mut document, "model", route).map_err(|error| error.to_string())?;
        fields.insert(
            "think",
            toml_edit::value(ThinkLevel::ProviderDefault.as_str()),
        );
        config::set_optional(
            fields,
            "reasoning_effort",
            (effort != ThinkLevel::ProviderDefault.as_str()).then_some(effort),
        );
    } else {
        document["default_model"] = toml_edit::value(route);
    }
    let resolved = config::save_model_document(&settings, &document, &[route])?;
    Ok(wait_for_catalog(connection, &resolved).await)
}

fn thinking_tier<'a>(
    settings: &'a Settings,
    tier: Option<&str>,
    route: &ResolvedModel,
) -> Option<&'a str> {
    let matches = |name: &str| {
        settings.resolved.tier(name).is_some_and(|candidate| {
            candidate.provider == route.connection && candidate.model == route.model
        })
    };
    if let Some(tier) = tier {
        return settings
            .resolved
            .tiers
            .get_key_value(tier)
            .filter(|(name, _)| matches(name))
            .map(|(name, _)| name.as_str());
    }
    if matches(&settings.default_tier) {
        return Some(&settings.default_tier);
    }
    let mut candidates = settings.resolved.tiers.keys().filter(|name| matches(name));
    candidates
        .next()
        .filter(|_| candidates.next().is_none())
        .map(String::as_str)
}

async fn active_settings(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
) -> Result<(ServiceIdentity, Settings), String> {
    let service = active_service(options, connection).await?;
    let settings = Settings::load_with_status(
        options.clone(),
        Some(service.config_path.clone()),
        &|name| std::env::var(name),
    )
    .map_err(|error| error.to_string())?;
    Ok((service, settings))
}

async fn wait_for_catalog(connection: Option<&Connection>, config: &kuku::config::Config) -> bool {
    let Some(connection) = connection else {
        return false;
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let catalog = connection.client.models().await;
            if catalog.is_ok_and(|catalog| {
                catalog.default_tier == config.default_tier
                    && catalog.tiers.len() == config.tiers.len()
                    && catalog.tiers.into_iter().all(|tier| {
                        let expected = config.tier(&tier.name);
                        expected.is_some_and(|route| {
                            route.provider == tier.connection && route.model == tier.model
                        }) && inspect_model(
                            config,
                            &AgentConfig {
                                tier: Some(tier.name),
                                ..AgentConfig::default()
                            },
                        )
                        .map_or_else(
                            |_| tier.error.is_some(),
                            |route| {
                                tier.settings == Some(route.settings)
                                    && tier.capabilities == Some(route.capabilities)
                                    && tier.output == Some(route.output)
                                    && tier.request_url == Some(route.request_url)
                            },
                        )
                    })
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(125)).await;
        }
    })
    .await
    .is_ok()
}
