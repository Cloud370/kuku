use kuku_client::{Action, AgentConfig, Error, ModelSelection};

use crate::jobs::{Job, Selection};
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::model_default::{PreparedDefaultSave, ResolvedModel};

use super::{App, Modal, PendingStage};

pub(super) struct PendingDefaultSave {
    ticket: u64,
    selection: Selection,
    plan: PreparedDefaultSave,
}

#[cfg(test)]
#[path = "model_default_tests.rs"]
mod tests;

impl App {
    pub(crate) fn resolved_model_route(&self) -> Result<ResolvedModel, &'static str> {
        let pane = self.pane();
        if pane.exists {
            let target = self.target(&self.active);
            if pane.waiting_for_configuration()
                || self.pending.values().any(|item| {
                    item.command.target == target
                        && matches!(item.command.action, Action::Configure { .. })
                        && !matches!(item.stage, PendingStage::Rejected(_))
                })
            {
                return Err("Wait for model settings to finish, then reopen /model.");
            }
            let snapshot = pane
                .snapshot
                .as_ref()
                .ok_or("Wait for the Agent's model selection to resolve, then reopen /model.")?;
            if snapshot.pending_selection_error.is_some() || snapshot.selection_error.is_some() {
                return Err("Wait for the Agent's model selection to resolve, then reopen /model.");
            }
            let selection = snapshot
                .pending_selection
                .as_ref()
                .or(snapshot.selection.as_ref())
                .ok_or("Resolve a model before setting it as the default.")?;
            Ok(ResolvedModel {
                connection: selection.connection.clone(),
                model: selection.model.clone(),
            })
        } else {
            if pane.model_problem.is_some() {
                return Err("Resolve the draft's model selection, then reopen /model.");
            }
            let selection = pane
                .initial_model
                .as_ref()
                .ok_or("Wait for the draft's model selection to resolve, then reopen /model.")?;
            Ok(ResolvedModel {
                connection: selection.connection.clone(),
                model: selection.model.clone(),
            })
        }
    }

    pub(crate) fn model_default_choice(&self) -> Choice {
        let detail = self
            .resolved_model_route()
            .map(|route| {
                format!(
                    "{}/{} · save as the persistent default",
                    route.connection, route.model
                )
            })
            .unwrap_or_else(str::to_owned);
        Choice::new(
            "Set current model as default",
            detail,
            ChoiceKind::SetCurrentModelAsDefault,
        )
    }

    pub(crate) fn prepare_model_default(&mut self) {
        if !self.require_service() {
            return;
        }
        let route = match self.resolved_model_route() {
            Ok(route) => route,
            Err(problem) => {
                self.problem(problem);
                return;
            }
        };
        self.pending_model_default = None;
        let selection = self.selection();
        let picker = self.picker(PickerKind::ModelDefault, Vec::new());
        picker.loading = true;
        picker.header = "Checking the active local service and config…".into();
        let ticket = picker.ticket.id;
        self.effects.push(Job::PrepareDefault {
            ticket,
            selection,
            route,
        });
    }

    pub(crate) fn confirm_model_default(&mut self, ticket: u64) {
        let Some(pending) = self.pending_model_default.take() else {
            self.problem("Default save is no longer available; reopen /model.");
            return;
        };
        if pending.ticket != ticket || pending.selection != self.selection() {
            self.problem("The selected Agent changed; reopen /model before saving.");
            return;
        }
        if !self
            .resolved_model_route()
            .is_ok_and(|route| route == *pending.plan.route())
        {
            self.problem("The model selection changed; reopen /model before saving.");
            return;
        }
        self.saving_model_default = Some((ticket, pending.selection.clone()));
        self.effects.push(Job::SaveDefault {
            ticket,
            selection: pending.selection,
            plan: pending.plan,
        });
    }

    pub(crate) fn cancel_model_default(&mut self, ticket: u64) {
        if self
            .pending_model_default
            .as_ref()
            .is_some_and(|pending| pending.ticket == ticket)
        {
            self.pending_model_default = None;
        }
    }

    pub(crate) fn clear_model_default_if_closed(&mut self) {
        let confirmation_open = matches!(
            &self.modal,
            Some(Modal::Picker(picker)) if matches!(&picker.kind, PickerKind::ModelDefault)
        );
        if !confirmation_open
            || self
                .pending_model_default
                .as_ref()
                .is_some_and(|pending| pending.selection != self.selection())
        {
            self.pending_model_default = None;
        }
    }

    pub(super) fn prepared_model_default(
        &mut self,
        ticket: u64,
        selection: Selection,
        result: Result<PreparedDefaultSave, String>,
    ) {
        let current_modal = matches!(
            self.modal,
            Some(Modal::Picker(ref picker))
                if matches!(picker.kind, PickerKind::ModelDefault) && picker.ticket.id == ticket
        );
        if !current_modal {
            return;
        }
        if selection != self.selection() {
            self.modal = None;
            self.problem("The selected Agent changed; reopen /model and try again.");
            return;
        }
        match result {
            Ok(plan)
                if self
                    .resolved_model_route()
                    .is_ok_and(|route| route == *plan.route()) =>
            {
                let header = plan.confirmation_header();
                let choices = vec![
                    Choice::new(
                        "Save as default",
                        "Write the confirmed config change",
                        ChoiceKind::ConfirmModelDefault,
                    ),
                    Choice::new(
                        "Cancel",
                        "No files are changed",
                        ChoiceKind::CancelModelDefault,
                    ),
                ];
                if let Some(Modal::Picker(picker)) = &mut self.modal {
                    picker.loading = false;
                    picker.problem = None;
                    picker.header = header;
                    picker.set_local(choices);
                    picker.selected = 1;
                }
                self.pending_model_default = Some(PendingDefaultSave {
                    ticket,
                    selection,
                    plan,
                });
            }
            Ok(_) => {
                self.modal = None;
                self.problem("The model selection changed; reopen /model and try again.");
            }
            Err(problem) => {
                if let Some(Modal::Picker(picker)) = &mut self.modal {
                    picker.loading = false;
                    picker.problem = Some(problem);
                    picker.header = "No files were changed · reopen /model to retry".into();
                }
            }
        }
    }

    pub(super) fn saved_model_default(
        &mut self,
        ticket: u64,
        selection: Selection,
        result: Result<(), String>,
    ) {
        let active_save =
            self.saving_model_default
                .as_ref()
                .is_some_and(|(active_ticket, active_selection)| {
                    *active_ticket == ticket && *active_selection == selection
                });
        if !active_save || selection != self.selection() {
            return;
        }
        self.saving_model_default = None;
        match result {
            Ok(()) => {
                self.confirmation("Saved as default · the current Session and Agent are unchanged")
            }
            Err(problem) => self.problem(problem),
        }
    }

    pub(crate) fn configure(&mut self, config: AgentConfig) {
        if self.pane().exists {
            let target = self.target(&self.active);
            if self.pending.values().any(|item| {
                item.command.target == target
                    && matches!(item.command.action, Action::Configure { .. })
                    && !matches!(item.stage, PendingStage::Rejected(_))
            }) {
                self.problem("Check the previous settings change in /pending");
                return;
            }
            self.issue(
                self.active.clone(),
                Action::Configure {
                    config: Box::new(config),
                },
            );
        } else {
            self.pane_mut().initial_config = config;
            self.preview_model();
        }
    }

    pub(super) fn preview_model(&mut self) {
        if self.require_service() {
            self.model_version = self.model_version.wrapping_add(1);
            let config = self.pane().initial_config.clone();
            self.pane_mut().initial_model = None;
            self.effects.push(Job::Model(config, self.model_version));
        }
    }

    pub(super) fn default_model_updated(
        &mut self,
        version: u64,
        result: Result<ModelSelection, Error>,
    ) {
        if version != self.model_version || self.pane().exists {
            return;
        }
        match result {
            Ok(model) => {
                let pane = self.pane_mut();
                pane.initial_model = Some(model);
                pane.model_problem = None;
            }
            Err(error) => self.pane_mut().model_problem = Some(error.to_string()),
        }
    }
}
