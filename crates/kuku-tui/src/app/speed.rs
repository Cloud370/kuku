use kuku::config::CustomServiceTier;
use kuku_client::{Action, AgentConfig, ModelSelection, Snapshot};

use crate::jobs::{Job, Selection};
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::setup::service_tier::{valid_tier, SpeedResult, SpeedWork};

use super::{Address, App, Modal, PendingStage};

const COMMON_TIERS: [&str; 3] = ["flex", "fast", "ultrafast"];

#[derive(Clone, Debug)]
pub(crate) struct SpeedTarget {
    pub(crate) origin: Selection,
    pub(crate) config: AgentConfig,
    pub(crate) model: ModelSelection,
}

impl SpeedTarget {
    pub(crate) fn from_snapshot(origin: Selection, snapshot: &Snapshot) -> Result<Self, String> {
        if snapshot.pending_selection_error.is_some() || snapshot.selection_error.is_some() {
            return Err("Resolve the main Agent's model before selecting a service tier.".into());
        }
        let model = snapshot
            .pending_selection
            .as_ref()
            .or(snapshot.selection.as_ref())
            .ok_or("Wait for the main Agent's model to resolve.")?;
        Ok(Self {
            origin,
            config: snapshot.agent.config.clone(),
            model: ModelSelection {
                request_url: model.request_url.clone(),
                connection: model.connection.clone(),
                protocol: model.protocol,
                model: model.model.clone(),
                context_window_tokens: model.context_window_tokens,
                max_output_tokens: model.max_output_tokens,
                settings: model.settings.clone(),
                capabilities: model.capabilities.clone(),
                output: model.output,
            },
        })
    }

    pub(crate) fn same_route(&self, model: &ModelSelection) -> bool {
        self.model.connection == model.connection
            && self.model.model == model.model
            && self.model.request_url == model.request_url
            && self.model.protocol == model.protocol
    }

    fn declared(&self, tier: &str) -> bool {
        use kuku::CapabilitySource::AdapterProfile;
        let caps = &self.model.capabilities;
        let common = COMMON_TIERS
            .iter()
            .any(|value| caps.equivalent_service_tiers(value, tier));
        caps.supports_service_tier(tier)
            && (!common
                || caps.service_tier_source != Some(AdapterProfile)
                || caps
                    .custom_service_tiers
                    .keys()
                    .any(|value| caps.equivalent_service_tiers(value, tier)))
    }

    fn choice(&self, tier: &str) -> Choice {
        let caps = &self.model.capabilities;
        let custom = caps
            .custom_service_tiers
            .iter()
            .find(|(value, _)| caps.equivalent_service_tiers(value, tier));
        let current = self
            .config
            .service_tier
            .as_deref()
            .is_some_and(|value| caps.equivalent_service_tiers(value, tier));
        let declared = self.declared(tier);
        let availability = match custom {
            Some((_, CustomServiceTier::Confirmed)) => "Custom · confirmed by probe",
            Some((_, CustomServiceTier::Unverified)) => "Custom · unverified",
            None if declared => "",
            None => "Upstream availability unconfirmed",
        };
        let detail = if current {
            ["Current", availability]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        } else {
            availability.into()
        };
        let action = if declared {
            SpeedAction::Apply(Some(tier.into()))
        } else {
            SpeedAction::Custom(tier.into())
        };
        speed_choice(tier, &detail, action)
    }

    fn choices(&self, query: &str) -> Vec<Choice> {
        let inherit = if self.config.service_tier.is_none() {
            self.model
                .settings
                .service_tier
                .as_deref()
                .unwrap_or("Provider default")
        } else {
            ""
        };
        let mut choices = vec![speed_choice(
            "Follow model default",
            inherit,
            SpeedAction::Apply(None),
        )];
        let caps = &self.model.capabilities;
        choices.extend(COMMON_TIERS.into_iter().map(|value| self.choice(value)));
        choices.extend(
            caps.service_tiers
                .iter()
                .filter(|tier| {
                    !COMMON_TIERS
                        .iter()
                        .any(|value| caps.equivalent_service_tiers(tier, value))
                })
                .map(|tier| self.choice(tier)),
        );
        if valid_tier(query)
            && !COMMON_TIERS.contains(&query)
            && !caps.service_tiers.iter().any(|value| value == query)
        {
            if self.declared(query) {
                choices.insert(0, self.choice(query));
            } else {
                choices.push(speed_choice(
                    &format!("Add custom tier · {query}"),
                    "Upstream availability unconfirmed",
                    SpeedAction::Custom(query.into()),
                ));
            }
        }
        choices
    }
}

#[derive(Debug, Clone)]
pub(crate) enum SpeedAction {
    Apply(Option<String>),
    Custom(String),
    Probe(String),
    UseUnverified(String),
}

fn speed_choice(label: &str, detail: &str, action: SpeedAction) -> Choice {
    Choice::new(label, detail, ChoiceKind::Speed(action))
}

impl App {
    pub(crate) fn open_speed(&mut self, requested: Option<String>) {
        if !self.require_service() {
            return;
        }
        let origin = self.selection();
        let main = Address::main(self.active.session.clone());
        if self.speed_change_pending(&main) {
            self.problem("Wait for the main Agent's settings to finish.");
            return;
        }
        if main != self.active {
            let target = self.target(&main);
            let picker = self.picker(PickerKind::Speed, Vec::new());
            picker.header = "Loading the main Agent's model…".into();
            picker.loading = true;
            let ticket = picker.ticket.id;
            self.effects
                .push(Job::LoadSpeed(ticket, origin, target, requested));
            return;
        }
        let route = if let Some(snapshot) = &self.pane().snapshot {
            SpeedTarget::from_snapshot(origin, snapshot).map(Box::new)
        } else if let Some(model) = &self.pane().initial_model {
            Ok(Box::new(SpeedTarget {
                origin,
                config: self.pane().config().clone(),
                model: model.clone(),
            }))
        } else {
            Err("Wait for the main Agent's model to resolve or choose a model with /model.".into())
        };
        self.show_speed(route, requested);
    }

    pub(super) fn speed_loaded(
        &mut self,
        ticket: u64,
        route: Result<Box<SpeedTarget>, String>,
        requested: Option<String>,
    ) {
        if matches!(&self.modal, Some(Modal::Picker(picker)) if picker.kind == PickerKind::Speed && picker.ticket.id == ticket)
        {
            self.show_speed(route, requested);
        }
    }

    fn show_speed(&mut self, route: Result<Box<SpeedTarget>, String>, requested: Option<String>) {
        let target = match route {
            Ok(target) => target,
            Err(error) => {
                self.close_picker();
                self.problem(error);
                return;
            }
        };
        match requested {
            Some(tier) if target.declared(&tier) => self.apply_speed(&target, Some(tier)),
            Some(tier) => self.confirm_custom_speed(target, tier, false),
            None => {
                let choices = target.choices("");
                let picker = self.picker(PickerKind::Speed, choices);
                picker.header = format!("main · {}", target.model.model);
                picker.speed = Some(target);
            }
        }
    }

    pub(crate) fn update_speed_choices(&mut self) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        if let Some(target) = &picker.speed {
            picker.selected = 0;
            picker.set_local(target.choices(picker.query.text().trim()));
        }
    }

    pub(crate) fn choose_speed(&mut self, ticket: u64, action: SpeedAction) {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return;
        };
        let Some(target) = picker.speed.as_ref().filter(|_| ticket == picker.ticket.id) else {
            return;
        };
        let target = target.clone();
        match action {
            SpeedAction::Apply(tier) => self.apply_speed(&target, tier),
            SpeedAction::Custom(tier) => self.confirm_custom_speed(target, tier, false),
            SpeedAction::Probe(tier) => self.start_speed_work(target, SpeedWork::Probe(tier)),
            SpeedAction::UseUnverified(tier) => {
                self.start_speed_work(target, SpeedWork::Save(tier, CustomServiceTier::Unverified))
            }
        }
    }

    fn confirm_custom_speed(&mut self, target: Box<SpeedTarget>, tier: String, accepted: bool) {
        if self.remote {
            self.close_picker();
            self.problem("Add custom service tiers in the TUI on the service host.");
            return;
        }
        let mut choices = Vec::new();
        if !accepted {
            choices.push(speed_choice(
                "Probe and use",
                "One short request · may use quota",
                SpeedAction::Probe(tier.clone()),
            ));
        }
        choices.push(speed_choice(
            if accepted {
                "Use without verification"
            } else {
                "Skip probe and use"
            },
            "Save and switch",
            SpeedAction::UseUnverified(tier.clone()),
        ));
        let picker = self.picker(PickerKind::CustomSpeed, choices);
        picker.header = if accepted {
            format!("{tier} · Request accepted, actual tier unconfirmed")
        } else {
            format!("{tier} · {}", target.model.model)
        };
        picker.speed = Some(target);
    }

    fn start_speed_work(&mut self, target: Box<SpeedTarget>, work: SpeedWork) {
        if !self.speed_target_current(&target) {
            self.close_picker();
            self.problem("The main Agent's settings changed. Reopen /speed.");
            return;
        }
        let picker = self.picker(PickerKind::CustomSpeed, Vec::new());
        picker.header = match &work {
            SpeedWork::Probe(tier) => format!("Probing {tier}…"),
            SpeedWork::Save(tier, _) => format!("Saving {tier}…"),
        };
        picker.loading = true;
        let ticket = picker.ticket.id;
        self.effects.push(Job::CustomSpeed(ticket, target, work));
    }

    pub(super) fn speed_work_finished(
        &mut self,
        ticket: u64,
        target: Box<SpeedTarget>,
        result: Result<SpeedResult, String>,
    ) {
        let open = matches!(&self.modal, Some(Modal::Picker(picker)) if picker.ticket.id == ticket);
        match result {
            Ok(SpeedResult::Confirmed(tier)) if open => {
                self.start_speed_work(target, SpeedWork::Save(tier, CustomServiceTier::Confirmed));
            }
            Ok(SpeedResult::Unconfirmed(tier)) if open => {
                self.confirm_custom_speed(target, tier, true)
            }
            Ok(SpeedResult::Other(actual)) if open => {
                self.speed_problem(format!(
                    "Provider reported {actual}; the requested tier was not confirmed."
                ));
            }
            Ok(SpeedResult::Saved(tier)) if open => self.apply_speed(&target, Some(tier)),
            Ok(SpeedResult::Saved(tier)) => {
                self.confirmation(format!("Saved custom service tier {tier}"))
            }
            Err(error) if open => self.speed_problem(error),
            Err(error) => self.problem(error),
            _ => {}
        }
    }

    fn speed_problem(&mut self, problem: String) {
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            picker.loading = false;
            picker.problem = Some(problem);
        }
    }

    fn apply_speed(&mut self, target: &SpeedTarget, tier: Option<String>) {
        self.close_picker();
        if !self.speed_target_current(target) {
            self.problem("The main Agent's settings changed. Reopen /speed.");
            return;
        }
        let label = tier.as_deref().unwrap_or("follow model default");
        let message = format!("main · Service tier · {label}");
        if target.config.service_tier != tier {
            let mut config = target.config.clone();
            config.service_tier = tier;
            let main = Address::main(self.active.session.clone());
            if self.active == main {
                self.configure(config);
            } else {
                self.issue(
                    main,
                    Action::Configure {
                        config: Box::new(config),
                    },
                );
            }
        }
        self.confirmation(message);
    }

    fn speed_change_pending(&self, main: &Address) -> bool {
        self.panes
            .get(main)
            .is_some_and(|pane| pane.waiting_for_configuration())
            || self.pending.values().any(|item| {
                item.command.target == self.target(main)
                    && matches!(item.command.action, Action::Configure { .. })
                    && !matches!(item.stage, PendingStage::Rejected(_))
            })
    }

    fn speed_target_current(&self, target: &SpeedTarget) -> bool {
        let main = Address::main(self.active.session.clone());
        target.origin == self.selection()
            && !self.speed_change_pending(&main)
            && (main != self.active
                || self.pane().config() == &target.config
                    && self.resolved_model_route().is_ok_and(|route| {
                        route.connection == target.model.connection
                            && route.model == target.model.model
                    }))
    }
}
