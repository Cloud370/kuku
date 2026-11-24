use kuku::config::{ModelRef, ThinkLevel};
use kuku::{CapabilitySource, ProviderCapabilities, ProviderSettings};

use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::setup::{ThinkingDefaults, ThinkingSaveTarget, ThinkingSetting};
use crate::text::effort_label;

use super::{App, Modal, Pane};

#[derive(Debug)]
pub(crate) struct ThinkingPicker {
    pub(crate) route: ModelRef,
    pub(crate) current: Option<String>,
    pub(crate) inherited: Option<String>,
    pub(crate) declared: Option<String>,
    pub(crate) unlisted_current: Option<String>,
    pub(crate) pending: bool,
    pub(crate) save: Option<ThinkingSaveTarget>,
    pub(crate) confirm: Option<String>,
}

impl Pane {
    pub(crate) fn effective_model_settings(
        &self,
    ) -> Option<(&ProviderSettings, &ProviderCapabilities)> {
        if let Some(snapshot) = &self.snapshot {
            snapshot
                .selection
                .as_ref()
                .map(|model| (&model.settings, &model.capabilities))
        } else {
            self.initial_model
                .as_ref()
                .map(|model| (&model.settings, &model.capabilities))
        }
    }

    pub(crate) fn next_thinking(&self) -> Option<Option<&str>> {
        let snapshot = self.snapshot.as_ref()?;
        let active = snapshot.selection.as_ref()?;
        let next = snapshot.pending_selection.as_ref()?;
        (active.connection == next.connection
            && active.model == next.model
            && active.protocol == next.protocol
            && active.request_url == next.request_url
            && active.output == next.output
            && active.context_window_tokens == next.context_window_tokens
            && active.max_output_tokens == next.max_output_tokens
            && active.settings.service_tier == next.settings.service_tier
            && active.settings.requested_service_tier == next.settings.requested_service_tier
            && active.tools == next.tools
            && active.settings.reasoning_effort != next.settings.reasoning_effort)
            .then_some(next.settings.reasoning_effort.as_deref())
    }
}

impl App {
    pub(crate) fn open_thinking(&mut self) {
        if !self.require_service() {
            return;
        }
        let Ok(route) = self.resolved_model_route() else {
            self.problem("Wait for the model to resolve or choose a route with /model");
            return;
        };
        let pane = self.pane();
        let Some((settings, capabilities)) = pane.reasoning() else {
            return;
        };
        let config = pane.config().clone();
        let current = config.reasoning_effort.clone();
        let inherited = current.is_none().then(|| inherited_label(settings));
        let declared = capabilities.reasoning_default.as_ref().map(|default| {
            let source = match default.source {
                CapabilitySource::ModelList | CapabilitySource::ProtocolModelList => {
                    "Provider declares"
                }
                CapabilitySource::Manual
                | CapabilitySource::ProtocolManual
                | CapabilitySource::AdapterProfile => "Declared default",
            };
            format!("{source} · {}", effort_label(Some(&default.value)))
        });
        let pending = pane
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.pending_selection.is_some());
        let choices = thinking_choices(capabilities);
        let unlisted_current = current.as_ref().filter(|effort| {
            !choices.iter().any(|choice| matches!(&choice.kind, ChoiceKind::Thinking(Some(level)) if level == *effort))
        }).map(|effort| effort_label(Some(effort)));
        let fetch_defaults = !self.remote || config.reasoning_effort.is_some();
        let picker = self.picker(PickerKind::Thinking, choices);
        picker.thinking = Some(Box::new(ThinkingPicker {
            route: route.clone(),
            current,
            inherited,
            declared,
            unlisted_current,
            pending,
            save: None,
            confirm: None,
        }));
        if fetch_defaults {
            let ticket = picker.ticket.id;
            picker.loading = true;
            self.effects
                .push(Job::ThinkingDefaults(ticket, config, route));
        }
    }

    pub(crate) fn cycle_thinking(&mut self) {
        if !self.require_service() || self.model_change_in_flight() {
            return;
        }
        if self.resolved_model_route().is_err() {
            self.problem("Wait for the current model settings to resolve");
            return;
        }
        let Some((settings, capabilities)) = self.pane().reasoning() else {
            return;
        };
        let levels = supported_levels(capabilities);
        let Some(first) = levels.first().copied() else {
            self.problem("No supported thinking levels are available for this model");
            return;
        };
        let next = levels
            .iter()
            .position(|effort| Some(*effort) == settings.reasoning_effort.as_deref())
            .and_then(|index| levels.get(index + 1).copied())
            .unwrap_or(first);
        if settings.reasoning_effort.as_deref() != Some(next) {
            self.apply_thinking(Some(next.to_owned()));
        }
    }

    pub(crate) fn apply_thinking_choice(&mut self, effort: Option<String>) {
        if self.thinking_choice_current(effort.as_deref()) {
            self.apply_thinking(effort);
        } else {
            self.problem("The selected route changed · reopen /thinking");
        }
    }

    pub(crate) fn save_thinking_default(&mut self) {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return;
        };
        let Some(thinking) = &picker.thinking else {
            return;
        };
        let (Some(effort), Some(target)) = (&thinking.confirm, &thinking.save) else {
            return;
        };
        if self.thinking_choice_current(Some(effort)) {
            let change = ThinkingSetting {
                target: target.clone(),
                effort: effort.clone(),
            };
            self.effects.push(Job::SaveModelSetting(change));
        } else {
            self.problem("The selected route changed · reopen /thinking");
        }
    }

    pub(super) fn thinking_defaults(&mut self, ticket: u64, defaults: ThinkingDefaults) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        if picker.ticket.id != ticket {
            return;
        }
        let has_levels = picker.choice_count() > 1;
        let Some(thinking) = &mut picker.thinking else {
            return;
        };
        picker.loading = false;
        if let Some(inherited) = defaults.inherited {
            thinking.inherited = Some(inherited_label(&inherited));
        }
        thinking.save = defaults.save.filter(|_| has_levels);
    }

    fn thinking_choice_current(&self, effort: Option<&str>) -> bool {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return false;
        };
        picker.thinking.as_ref().is_some_and(|thinking| {
            self.available()
                && self.resolved_model_route().as_ref() == Ok(&thinking.route)
                && effort.is_none_or(|effort| {
                    self.pane().reasoning().is_some_and(|(_, caps)| {
                        caps.selectable_reasoning_efforts()
                            .any(|level| level == effort)
                    })
                })
        })
    }

    fn apply_thinking(&mut self, effort: Option<String>) {
        let mut config = self.pane().config().clone();
        if config.reasoning_effort != effort {
            config.reasoning_effort = effort;
            self.configure(config);
        }
    }
}

fn thinking_choices(capabilities: &ProviderCapabilities) -> Vec<Choice> {
    let defaults = Choice::new("Use default", "", ChoiceKind::Thinking(None));
    let levels = supported_levels(capabilities).into_iter().map(|level| {
        Choice::new(
            effort_label(Some(level)),
            "",
            ChoiceKind::Thinking(Some(level.into())),
        )
    });
    std::iter::once(defaults).chain(levels).collect()
}

fn supported_levels(capabilities: &ProviderCapabilities) -> Vec<&str> {
    let mut levels = Vec::new();
    for level in capabilities.selectable_reasoning_efforts() {
        if level != ThinkLevel::ProviderDefault.as_str() && !levels.contains(&level) {
            levels.push(level);
        }
    }
    levels
}

fn inherited_label(settings: &ProviderSettings) -> String {
    settings.reasoning_effort.as_deref().map_or_else(
        || "Provider selected".into(),
        |effort| effort_label(Some(effort)),
    )
}
