use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Modal};
use crate::keys::Shortcut;
use crate::menu::ChoiceKind;

use super::is_text;

impl App {
    pub(super) fn thinking_picker_key(&mut self, key: KeyEvent) -> bool {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return false;
        };
        let Some(thinking) = &picker.thinking else {
            return false;
        };
        if thinking.confirm.is_some() {
            if Shortcut::ENTER.matches(key) {
                self.save_thinking_default();
                self.close_picker();
            }
            return true;
        }
        if matches!(key.code, KeyCode::Char('d' | 'D')) && is_text(key) {
            if thinking.save.is_some() {
                if let Some(ChoiceKind::Thinking(Some(effort))) = picker.selected_choice() {
                    if let Some(thinking) = &mut picker.thinking {
                        thinking.confirm = Some(effort);
                    }
                }
            }
            return true;
        }
        false
    }

    pub(super) fn back_from_thinking_confirmation(&mut self) -> bool {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return false;
        };
        picker
            .thinking
            .as_mut()
            .is_some_and(|thinking| thinking.confirm.take().is_some())
    }
}
