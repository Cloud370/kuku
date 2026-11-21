use kuku_client::{Action, AgentConfig, Error, ModelCatalog, TierSummary};

use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind, PromptKind};
use crate::text::{effort_label, service_tier_label, single_line};

use super::{Address, App, Modal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentSetupStep {
    Review,
    Model,
    Thinking,
    Service,
}

impl AgentSetupStep {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Review => "Create Agent",
            Self::Model => "Agent model",
            Self::Thinking => "Agent thinking",
            Self::Service => "Agent service tier",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AgentSetupAction {
    Create,
    Open(AgentSetupStep),
    Model(Option<String>),
    Thinking(Option<String>),
    Service(Option<String>),
}

#[derive(Debug)]
pub(crate) struct AgentSetup {
    address: Address,
    config: AgentConfig,
    models: Option<ModelCatalog>,
}

impl AgentSetup {
    fn route(&self) -> Option<&TierSummary> {
        let catalog = self.models.as_ref()?;
        let tier = self.config.tier.as_deref().unwrap_or(&catalog.default_tier);
        catalog.tiers.iter().find(|route| route.name == tier)
    }

    fn choices(&self, step: AgentSetupStep) -> Vec<Choice> {
        let route = self.route();
        let settings = route.and_then(|route| route.settings.as_ref());
        let caps = route.and_then(|route| route.capabilities.as_ref());
        let thinking = caps
            .into_iter()
            .flat_map(|caps| caps.selectable_reasoning_efforts());
        let services = caps.into_iter().flat_map(|caps| &caps.service_tiers);
        let route_thinking = settings
            .and_then(|settings| settings.reasoning_effort.as_deref())
            .or_else(|| {
                caps.and_then(|caps| {
                    caps.reasoning_default
                        .as_ref()
                        .map(|value| value.value.as_str())
                })
            });
        let route_service = settings.and_then(|settings| settings.service_tier.as_deref());
        match step {
            AgentSetupStep::Review => {
                let mut choices = vec![choice("Create Agent", "", AgentSetupAction::Create)];
                if self.models.is_some() {
                    choices.push(choice(
                        "Model",
                        route.map_or("Default", |route| &route.model),
                        AgentSetupAction::Open(AgentSetupStep::Model),
                    ));
                }
                if thinking.count() > 0 {
                    choices.push(choice(
                        "Thinking",
                        effort_label(self.config.reasoning_effort.as_deref().or(route_thinking)),
                        AgentSetupAction::Open(AgentSetupStep::Thinking),
                    ));
                }
                if services.count() > 0 {
                    choices.push(choice(
                        "Service tier",
                        service_tier_label(self.config.service_tier.as_deref().or(route_service)),
                        AgentSetupAction::Open(AgentSetupStep::Service),
                    ));
                }
                choices
            }
            AgentSetupStep::Model => {
                let Some(catalog) = &self.models else {
                    return Vec::new();
                };
                let default = catalog
                    .tiers
                    .iter()
                    .find(|route| route.name == catalog.default_tier);
                std::iter::once(choice(
                    "Default",
                    default.map_or("", |route| &route.model),
                    AgentSetupAction::Model(None),
                ))
                .chain(
                    catalog
                        .tiers
                        .iter()
                        .filter(|route| route.error.is_none())
                        .map(|route| {
                            choice(
                                &route.name,
                                &route.model,
                                AgentSetupAction::Model(Some(route.name.clone())),
                            )
                        }),
                )
                .collect()
            }
            AgentSetupStep::Thinking => std::iter::once(choice(
                "Use model default",
                effort_label(route_thinking),
                AgentSetupAction::Thinking(None),
            ))
            .chain(thinking.map(|value| {
                choice(
                    effort_label(Some(value)),
                    "",
                    AgentSetupAction::Thinking(Some(value.to_owned())),
                )
            }))
            .collect(),
            AgentSetupStep::Service => std::iter::once(choice(
                "Use model default",
                service_tier_label(route_service),
                AgentSetupAction::Service(None),
            ))
            .chain(services.map(|value| {
                choice(
                    service_tier_label(Some(value)),
                    "",
                    AgentSetupAction::Service(Some(value.clone())),
                )
            }))
            .collect(),
        }
    }

    fn select_model(&mut self, tier: Option<String>) {
        self.config.tier = tier;
        let caps = self.route().and_then(|route| route.capabilities.as_ref());
        let keep_thinking = self.config.reasoning_effort.as_ref().is_none_or(|value| {
            caps.is_some_and(|caps| {
                caps.selectable_reasoning_efforts()
                    .any(|option| option == value)
            })
        });
        let keep_service = self
            .config
            .service_tier
            .as_ref()
            .is_none_or(|value| caps.is_some_and(|caps| caps.service_tiers.contains(value)));
        if !keep_thinking {
            self.config.reasoning_effort = None;
        }
        if !keep_service {
            self.config.service_tier = None;
        }
    }
}

fn choice(label: impl Into<String>, detail: impl Into<String>, action: AgentSetupAction) -> Choice {
    Choice::new(label, detail, ChoiceKind::AgentSetup(action))
}

impl App {
    pub(crate) fn begin_agent_setup(&mut self, name: String) {
        if !self.require_service() {
            return;
        }
        let setup = Box::new(AgentSetup {
            address: Address::new(self.active.session.clone(), name),
            config: AgentConfig::default(),
            models: None,
        });
        let picker = self.picker(PickerKind::AgentSetup(AgentSetupStep::Review), Vec::new());
        picker.header = format!("{} · Loading models…", single_line(&setup.address.agent));
        picker.loading = true;
        picker.agent_setup = Some(setup);
        let ticket = picker.ticket.id;
        self.effects.push(Job::AgentModels(ticket));
    }

    pub(super) fn agent_models_loaded(&mut self, ticket: u64, result: Result<ModelCatalog, Error>) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        if picker.ticket.id != ticket {
            return;
        }
        let Some(mut setup) = picker.agent_setup.take() else {
            return;
        };
        let problem = match result {
            Ok(catalog) => {
                setup.models = Some(catalog);
                None
            }
            Err(error) => Some(error.to_string()),
        };
        self.show_agent_setup(AgentSetupStep::Review, setup);
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            picker.problem = problem;
        }
    }

    pub(crate) fn choose_agent_setup(&mut self, action: AgentSetupAction) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let Some(mut setup) = picker.agent_setup.take() else {
            return;
        };
        match action {
            AgentSetupAction::Create => {
                self.modal = None;
                self.modal_selection.clear();
                self.issue(
                    setup.address,
                    Action::CreateAgent {
                        config: Box::new(setup.config),
                    },
                );
                return;
            }
            AgentSetupAction::Open(step) => {
                self.show_agent_setup(step, setup);
                return;
            }
            AgentSetupAction::Model(tier) => setup.select_model(tier),
            AgentSetupAction::Thinking(value) => setup.config.reasoning_effort = value,
            AgentSetupAction::Service(value) => setup.config.service_tier = value,
        }
        self.show_agent_setup(AgentSetupStep::Review, setup);
    }

    pub(crate) fn back_from_agent_setup(&mut self) -> bool {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return false;
        };
        let PickerKind::AgentSetup(step) = picker.kind else {
            return false;
        };
        let Some(setup) = picker.agent_setup.take() else {
            return false;
        };
        if step == AgentSetupStep::Review {
            self.prompt(PromptKind::Agent, "New Agent", "Choose the Agent's address");
            if let Some(Modal::Prompt(prompt)) = &mut self.modal {
                prompt.editor.set_text(setup.address.agent);
            }
        } else {
            self.show_agent_setup(AgentSetupStep::Review, setup);
        }
        true
    }

    fn show_agent_setup(&mut self, step: AgentSetupStep, setup: Box<AgentSetup>) {
        self.modal_selection.clear();
        let model_problem = setup
            .route()
            .and_then(|route| route.error.as_ref())
            .map(|error| error.message.clone());
        let picker = self.picker(PickerKind::AgentSetup(step), setup.choices(step));
        picker.header = format!("Agent {}", single_line(&setup.address.agent));
        if step == AgentSetupStep::Review {
            picker.problem = model_problem;
        }
        picker.agent_setup = Some(setup);
    }
}
