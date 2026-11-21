use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Modal};
use crate::keys::Shortcut;
use crate::menu::{ChoiceKind, PickerKind};

impl App {
    pub(super) fn picker_key(&mut self, key: KeyEvent) {
        if self.thinking_picker_key(key) || self.rewind_picker_key(key) {
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
                if let Some(choice) = picker.selected_choice() {
                    let kind = picker.kind.clone();
                    let ticket = picker.ticket.id;
                    self.choose(kind, choice, ticket);
                }
            }
            key if Shortcut::TAB.matches(key) && picker.kind.completes_text() => {
                match (&picker.kind, picker.selected_choice()) {
                    (PickerKind::Commands, Some(ChoiceKind::Command(command))) => {
                        self.complete_command(command);
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
            key if Shortcut::UP.matches(key) => picker.cycle_by(-1),
            key if Shortcut::DOWN.matches(key) => picker.cycle_by(1),
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
                self.picker_query_changed();
            }
            _ => {}
        }
    }

    pub(super) fn picker_query_changed(&mut self) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        if picker.kind == PickerKind::Commands && !picker.query.text().starts_with('/') {
            self.close_picker();
            self.focus_composer();
        } else {
            picker.selected = 0;
            self.search(false);
        }
    }
}
