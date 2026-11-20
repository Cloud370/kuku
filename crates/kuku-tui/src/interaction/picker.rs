use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, JobCommand, Modal};
use crate::jobs::Job;
use crate::keys::Shortcut;
use crate::menu::{ChoiceKind, PickerKind};

impl App {
    pub(super) fn picker_key(&mut self, key: KeyEvent) {
        if self.rewind_picker_key(key) {
            return;
        }
        let page = self.modal_page as isize;
        let details_page = self.modal_info_rows.max(1) as isize;
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        if matches!(picker.kind, PickerKind::MessageActions) {
            match key.code {
                KeyCode::Tab | KeyCode::Right if key.modifiers.is_empty() => {
                    picker.cycle_by(1);
                    return;
                }
                KeyCode::Left if key.modifiers.is_empty() => {
                    picker.cycle_by(-1);
                    return;
                }
                _ => {}
            }
        }
        match key {
            key if Shortcut::ENTER.matches(key) => {
                if matches!(picker.kind, PickerKind::Commands) {
                    if let Some(command) = JobCommand::parse(picker.query.text()) {
                        if matches!(command, JobCommand::Invalid) {
                            picker.problem = Some("Use /jobs or /jobs cancel <id>".into());
                            return;
                        }
                        self.modal = None;
                        self.effects.push(Job::ClosePicker);
                        self.run_job_command(command);
                        return;
                    }
                }
                if let Some(choice) = picker.selected_choice() {
                    let kind = picker.kind.clone();
                    let ticket = picker.ticket.id;
                    self.choose(kind, choice, ticket);
                }
            }
            key if Shortcut::TAB.matches(key) && picker.kind.completes_text() => {
                match (&picker.kind, picker.selected_choice()) {
                    (PickerKind::Commands, Some(ChoiceKind::Command(command))) => {
                        let completed = format!("/{}", command.name());
                        if picker.query.text() != completed {
                            picker.query.set_text(completed);
                            picker.selected = 0;
                            self.search(false);
                        }
                    }
                    (PickerKind::Commands, Some(choice @ ChoiceKind::Skill(_)))
                    | (PickerKind::Paths(_), Some(choice @ ChoiceKind::Path(_))) => {
                        let kind = picker.kind.clone();
                        let ticket = picker.ticket.id;
                        self.choose(kind, choice, ticket);
                    }
                    _ => {}
                }
            }
            key if Shortcut::UP.matches(key) => picker.move_by(-1),
            key if Shortcut::DOWN.matches(key) => picker.move_by(1),
            key if Shortcut::PAGE_UP.matches(key)
                && matches!(picker.kind, PickerKind::Server | PickerKind::ModelDefault) =>
            {
                picker.header_top = picker.header_top.saturating_add_signed(-details_page);
            }
            key if Shortcut::PAGE_DOWN.matches(key)
                && matches!(picker.kind, PickerKind::Server | PickerKind::ModelDefault) =>
            {
                picker.header_top = picker.header_top.saturating_add_signed(details_page);
            }
            key if Shortcut::PAGE_UP.matches(key) => picker.move_by(-page),
            key if Shortcut::PAGE_DOWN.matches(key) => picker.move_by(page),
            _ if picker.kind.searchable()
                && super::edit(&mut picker.query, key, self.modal_editor_width, false) =>
            {
                picker.selected = 0;
                self.search(false);
            }
            _ => {}
        }
    }
}
