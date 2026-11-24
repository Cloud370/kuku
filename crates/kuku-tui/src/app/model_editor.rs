use crossterm::event::Event;

use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::setup::{ModelEditor, ModelEditorAction, SetupIntent};

use super::{App, Modal};

impl App {
    pub(super) fn need_model_setup(&mut self) {
        self.connecting = false;
        self.setup_required = true;
        self.connection_problem = None;
    }

    pub(crate) fn open_models(&mut self) {
        if self.setup_required {
            self.open_model_editor(SetupIntent::Default);
            return;
        }
        self.clear_model_default_if_closed();
        let save_default = self.model_default_choice();
        self.picker(
            PickerKind::Models,
            vec![
                Choice::new(
                    "Add model or connection",
                    "",
                    ChoiceKind::EditModel(SetupIntent::Add),
                ),
                save_default,
            ],
        );
        self.search(true);
    }

    pub(crate) fn use_model_route(&mut self, tier: &kuku_client::TierSummary) {
        if let Some(error) = &tier.error {
            self.problem(error.message.clone());
            return;
        }
        let mut config = self.route_config();
        config.tier = Some(tier.name.clone());
        self.configure(config);
    }

    pub(crate) fn save_and_use_model(&mut self, tier: Box<kuku_client::TierSummary>) {
        if self.remote {
            self.problem("Change the default model on the service host.");
            return;
        }
        if let Some(error) = &tier.error {
            self.problem(error.message.clone());
            return;
        }
        if !self.require_service() {
            return;
        }
        if self.saving_default_and_switch.is_some() || self.model_change_in_flight() {
            self.problem("Wait for the previous settings change to be resolved.");
            return;
        }
        let selection = self.selection();
        self.saving_default_and_switch = Some(selection.clone());
        self.effects.push(Job::SaveAndUseModel(selection, tier));
    }

    pub(super) fn default_switch_pending(&self) -> bool {
        self.saving_default_and_switch
            .as_ref()
            .is_some_and(|selection| selection.target == self.target(&self.active))
    }

    pub(super) fn model_default_and_switch_saved(
        &mut self,
        selection: crate::jobs::Selection,
        tier: Box<kuku_client::TierSummary>,
        result: Result<bool, String>,
    ) {
        if self.saving_default_and_switch.as_ref() != Some(&selection) {
            return;
        }
        self.saving_default_and_switch = None;
        if matches!(result, Ok(true)) && selection == self.selection() {
            if self.model_change_in_flight() {
                self.message(format!(
                    "Saved {} as default · wait for the pending model change",
                    tier.model
                ));
                return;
            }
            self.use_model_route(&tier);
            self.confirmation(format!("Saved {} as default and switched", tier.model));
        } else if matches!(result, Ok(false)) && selection == self.selection() {
            self.problem("Saved the default model · the service timed out before switching");
        } else {
            self.model_setting_saved(result);
        }
    }

    pub(crate) fn open_model_editor(&mut self, intent: SetupIntent) {
        if self.remote {
            self.problem("Open kuku init on the service host to change model settings.");
            return;
        }
        if !self.setup_required && !self.require_service() {
            return;
        }
        let picker = self.picker(PickerKind::ModelActions, Vec::new());
        picker.loading = true;
        picker.header = "Opening shared model settings…".into();
        let ticket = picker.ticket.id;
        self.effects.push(Job::OpenModelEditor(ticket, intent));
    }

    pub(crate) fn model_editor_input(&mut self, event: Event) {
        let Some(mut editor) = self.model_editor.take() else {
            return;
        };
        match editor.handle(event) {
            ModelEditorAction::None => {}
            ModelEditorAction::Cancel => {
                if !self.setup_required {
                    self.open_models();
                }
                return;
            }
            ModelEditorAction::Save => {
                let picker = self.picker(PickerKind::ModelActions, Vec::new());
                picker.loading = true;
                picker.header = "Saving the active service configuration…".into();
                let ticket = picker.ticket.id;
                self.effects.push(Job::SaveModelEditor(ticket, editor));
                return;
            }
        }
        self.model_editor = Some(editor);
    }

    pub(super) fn model_editor_opened(
        &mut self,
        ticket: u64,
        result: Result<Box<ModelEditor>, String>,
    ) {
        if !self.model_editor_current(ticket) {
            return;
        }
        match result {
            Ok(editor) => {
                self.modal = None;
                self.model_editor = Some(editor);
            }
            Err(error) => {
                if self.setup_required {
                    self.modal = None;
                } else {
                    self.open_models();
                }
                self.problem(error);
            }
        }
    }

    pub(super) fn model_editor_saved(
        &mut self,
        ticket: u64,
        mut editor: Box<ModelEditor>,
        result: Result<bool, String>,
    ) {
        let current = self.model_editor_current(ticket);
        match result {
            Ok(_) if editor.is_first_run() => {
                if current {
                    self.modal = None;
                }
                self.reconnect();
            }
            Ok(applied) => {
                if current {
                    self.open_models();
                }
                self.model_setting_saved(Ok(applied));
            }
            Err(problem) if current => {
                editor.failed(problem);
                self.modal = None;
                self.model_editor = Some(editor);
            }
            Err(problem) => self.problem(problem),
        }
    }

    pub(super) fn model_setting_saved(&mut self, result: Result<bool, String>) {
        match result {
            Ok(true) => {
                if self.pane().exists {
                    self.effects.push(Job::Refresh(self.selection()));
                } else {
                    self.preview_model();
                }
                self.confirmation("Saved model settings");
            }
            Ok(false) => {
                self.message("Saved model settings · the kuku service is still applying them")
            }
            Err(problem) => self.problem(problem),
        }
    }

    fn model_editor_current(&self, ticket: u64) -> bool {
        matches!(&self.modal, Some(Modal::Picker(picker))
            if picker.kind == PickerKind::ModelActions && picker.ticket.id == ticket)
    }
}
