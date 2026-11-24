use kuku::config::ModelRef;
use kuku_client::{Action, AgentConfig, Error, ModelCatalog, ModelSummary};

use crate::jobs::{Job, subscription_labels};
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
    Model(Option<ModelRef>),
    Thinking(Option<String>),
    Service(Option<String>),
}

#[derive(Debug)]
pub(crate) struct AgentSetup {
    address: Address,
    config: AgentConfig,
    models: Option<ModelCatalog>,
    session_exists: bool,
    session_default_model: Option<ModelRef>,
}

impl AgentSetup {
    fn default_model(&self) -> Option<&ModelRef> {
        if self.session_exists {
            self.session_default_model.as_ref()
        } else {
            Some(&self.models.as_ref()?.default_model)
        }
    }

    fn route(&self) -> Option<&ModelSummary> {
        let catalog = self.models.as_ref()?;
        let exact = self
            .config
            .connection
            .as_deref()
            .zip(self.config.model.as_deref());
        let (connection, model) = match exact {
            Some(pair) => pair,
            None => {
                let default = self.default_model()?;
                (default.connection.as_str(), default.model.as_str())
            }
        };
        catalog
            .models
            .iter()
            .find(|route| route.connection == connection && route.model == model)
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
        let labels = self
            .models
            .as_ref()
            .map(|catalog| subscription_labels(&catalog.connections));
        let connection_label = |connection: &str| {
            labels
                .as_ref()
                .and_then(|labels| labels.get(connection))
                .cloned()
                .unwrap_or_else(|| connection.to_owned())
        };
        let label =
            |model: &str, connection: &str| format!("{model} · {}", connection_label(connection));
        match step {
            AgentSetupStep::Review => {
                let mut choices = vec![choice("Create Agent", "", AgentSetupAction::Create)];
                if self.models.is_some() {
                    choices.push(choice(
                        "Model",
                        route.map_or_else(
                            || "Session default".into(),
                            |route| label(&route.model, &route.connection),
                        ),
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
                std::iter::once(choice(
                    "Session default",
                    self.default_model()
                        .map_or_else(String::new, |id| label(&id.model, &id.connection)),
                    AgentSetupAction::Model(None),
                ))
                .chain(
                    catalog
                        .models
                        .iter()
                        .filter(|route| route.error.is_none())
                        .map(|route| {
                            choice(
                                &route.model,
                                connection_label(&route.connection),
                                AgentSetupAction::Model(Some(ModelRef::new(
                                    &route.connection,
                                    &route.model,
                                ))),
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

    fn select_model(&mut self, model: Option<ModelRef>) {
        (self.config.connection, self.config.model) =
            model.map_or((None, None), |id| (Some(id.connection), Some(id.model)));
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
            session_exists: self.pane().exists,
            session_default_model: self
                .pane()
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.session_default_model.clone()),
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
            AgentSetupAction::Model(model) => setup.select_model(model),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Address, AgentConfig, AgentSetup, AgentSetupAction, AgentSetupStep, ModelRef};
    use crate::menu::ChoiceKind;

    #[test]
    fn agent_creation_uses_the_existing_session_default_among_same_ids() {
        let catalog = serde_json::from_value(json!({
            "default_model": {"connection": "new", "model": "same/model"},
            "connections": [
                {"name":"new", "protocol":"openai-responses"},
                {"name":"original", "protocol":"openai-responses"},
            ],
            "models": [
                {"connection":"new", "model":"same/model"},
                {"connection":"original", "model":"same/model"},
            ],
        }))
        .unwrap();
        let mut setup = AgentSetup {
            address: Address::new("session".into(), "new-agent".into()),
            config: AgentConfig::default(),
            models: Some(catalog),
            session_exists: true,
            session_default_model: Some(ModelRef::new("original", "same/model")),
        };
        let choices = setup.choices(AgentSetupStep::Model);
        assert_eq!(choices[0].detail, "same/model · original");
        assert_eq!(setup.route().unwrap().connection, "original");
        let ChoiceKind::AgentSetup(AgentSetupAction::Model(new)) = &choices[1].kind else {
            panic!("expected the exact pair for the explicit model row");
        };
        setup.select_model(new.clone());
        assert_eq!(setup.route().unwrap().connection, "new");
        assert_eq!(setup.config.connection.as_deref(), Some("new"));
        assert_eq!(setup.config.model.as_deref(), Some("same/model"));
    }
}
