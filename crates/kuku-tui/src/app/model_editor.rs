use crossterm::event::Event;
use kuku_client::ModelSummary;

use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::setup::{ModelEditor, ModelEditorAction, SetupIntent};

use super::{App, Modal};

impl App {
    pub(super) fn need_model_setup(&mut self) {
        self.connecting = false;
        self.setup_required = true;
        self.connection_problem = None;
        self.open_models();
    }

    pub(crate) fn open_models(&mut self) {
        if self.setup_required {
            self.open_model_editor(SetupIntent::Default);
            return;
        }
        self.clear_model_default_if_closed();
        let actions = if self.remote {
            Vec::new()
        } else {
            vec![
                Choice::new(
                    "Add model or connection",
                    "",
                    ChoiceKind::EditModel(SetupIntent::Add),
                ),
                self.model_default_choice(),
            ]
        };
        self.picker(PickerKind::Models, actions);
        self.search(true);
    }

    pub(crate) fn use_model_route(&mut self, model: &ModelSummary) {
        if let Some(error) = &model.error {
            self.problem(error.message.clone());
            return;
        }
        let same_model = self.resolved_model_route().is_ok_and(|current| {
            current.connection == model.connection && current.model == model.model
        });
        let current = self.pane().config();
        let mut config = if same_model {
            if current.connection.as_ref() == Some(&model.connection)
                && current.model.as_ref() == Some(&model.model)
            {
                return;
            }
            current.clone()
        } else {
            self.route_config()
        };
        config.connection = Some(model.connection.clone());
        config.model = Some(model.model.clone());
        self.configure(config);
    }

    pub(crate) fn open_model_editor(&mut self, intent: SetupIntent) {
        if self.remote {
            self.problem(crate::local_settings::LOCAL_SERVICE_REQUIRED);
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
                    if editor.exit_after_save() {
                        self.exit();
                        return;
                    }
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
