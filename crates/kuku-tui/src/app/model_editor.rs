use crossterm::event::Event;

use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::setup::{ModelEditor, ModelEditorAction, SavedModelSetting, SetupIntent};

use super::{App, Modal};

impl App {
    pub(crate) fn open_models(&mut self) {
        self.clear_model_default_if_closed();
        let save_default = self.model_default_choice();
        self.picker(
            PickerKind::Models,
            vec![
                Choice::new(
                    "Add model or connection",
                    "Choose a saved connection or configure a new endpoint",
                    ChoiceKind::EditModel(SetupIntent::Add),
                ),
                save_default,
            ],
        );
        self.search(true);
    }

    pub(crate) fn open_model_actions(&mut self, tier: kuku_client::TierSummary) {
        let mut choices = Vec::new();
        if tier.error.is_none() {
            choices.push(Choice::new(
                "Use in this Session",
                "Choose this route for the selected Agent",
                ChoiceKind::Tier(tier.name.clone()),
            ));
        }
        choices.push(Choice::new(
            "Edit",
            "Open the model and its connection in shared settings",
            ChoiceKind::EditModel(SetupIntent::Edit(tier.name.clone())),
        ));
        if tier.error.is_none() {
            choices.push(Choice::new(
                "Set as default",
                "Save this route for new Sessions",
                ChoiceKind::ConfirmModelSetting(SavedModelSetting::Default(tier.name.clone())),
            ));
        }
        let mut header = format!("{} · {} · {}", tier.name, tier.connection, tier.model);
        if let Some(settings) = tier.settings {
            header.push_str(&format!(
                "
Thinking · {}",
                settings
                    .reasoning_effort
                    .as_deref()
                    .unwrap_or("Provider default (omit effort)")
            ));
        }
        if let Some(capabilities) = tier.capabilities {
            fn describe(value: Option<&kuku::DeclaredCapability<Vec<String>>>) -> String {
                value.map_or_else(|| "unknown".into(), |value| value.value.join(", "))
            }
            header.push_str(&format!(
                "
Input · {}   Output · {}   Interfaces · {}",
                describe(capabilities.input_modalities.as_ref()),
                describe(capabilities.output_modalities.as_ref()),
                describe(capabilities.interfaces.as_ref()),
            ));
        }
        if tier.output == Some(kuku::config::RequestedOutput::Image) {
            header.push_str(
                "
Agent requests image · built-in adapter supports text",
            );
        }
        self.picker(PickerKind::ModelActions, choices).header = header;
    }

    pub(crate) fn open_model_editor(&mut self, intent: SetupIntent) {
        if self.remote {
            self.problem("Open kuku init on the service host to change model settings.");
            return;
        }
        if !self.require_service() {
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
                self.open_models();
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
                self.open_models();
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

    pub(crate) fn confirm_model_setting(&mut self, change: SavedModelSetting) {
        if self.remote {
            self.problem("Change persistent model settings on the service host.");
            return;
        }
        let header = match &change {
            SavedModelSetting::Default(tier) => {
                format!("Save {tier} as the persistent default for new Sessions?")
            }
            SavedModelSetting::Thinking { model, effort, .. } => format!(
                "Save thinking for {model}? {}",
                if effort == "provider-default" {
                    "Omit effort in model requests".into()
                } else {
                    format!("Send {effort} in model requests")
                },
            ),
        };
        let picker = self.picker(
            PickerKind::ModelActions,
            vec![
                Choice::new(
                    "Save",
                    "Write the active service configuration",
                    ChoiceKind::SaveModelSetting(change),
                ),
                Choice::new("Cancel", "", ChoiceKind::Close),
            ],
        );
        picker.header = header;
        picker.selected = 1;
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
