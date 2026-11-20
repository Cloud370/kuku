use crossterm::event::KeyEvent;

use super::{App, Job, Modal, PromptKind, Shortcut};
use crate::menu::{ChoiceKind, PickerKind};

impl App {
    pub(super) fn key(&mut self, key: KeyEvent) {
        self.dismiss_confirmation();
        self.text_selection.cancel_drag();
        if !Shortcut::ESCAPE.matches(key) {
            self.escape_armed_at = None;
        }
        if Shortcut::ESCAPE.matches(key) {
            let selected_message = match &self.modal {
                Some(Modal::Picker(picker))
                    if matches!(picker.kind, PickerKind::MessageActions) =>
                {
                    match picker.selected_choice() {
                        Some(ChoiceKind::MessageAction { message, .. }) => Some(message),
                        Some(ChoiceKind::Detail {
                            message: Some(message),
                            ..
                        }) => Some(message),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(message) = selected_message {
                self.back_to_messages(message);
                self.escape_armed_at = None;
                return;
            }
            if self.modal.is_none() && self.pane().transcript.focused_rewind_group().is_some() {
                self.deactivate_context_focus();
                self.focus_composer();
                self.escape_armed_at = None;
                return;
            }
            let messages_picker = matches!(
                &self.modal,
                Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Messages)
            );
            if let Some(modal) = self.modal.take() {
                let close_picker = !matches!(&modal, Modal::Help);
                if let Modal::Prompt(prompt) = modal {
                    if let PromptKind::Question { target, request_id } = prompt.kind {
                        self.save_question_draft(&target, &request_id, prompt.editor);
                    }
                }
                if close_picker {
                    self.effects.push(Job::ClosePicker);
                }
                if messages_picker {
                    self.deactivate_context_focus();
                    self.focus_composer();
                }
                self.rewind_restore_files = false;
                self.escape_armed_at = None;
                return;
            }
            if self.pane_mut().transcript.close_notes() {
                self.escape_armed_at = None;
                return;
            }
            if self.pane().keyboard_focus == crate::app::KeyboardFocus::Transcript
                && self.pane().transcript.is_reading()
                && !self.pane().draft.text().is_empty()
            {
                self.deactivate_context_focus();
                self.focus_composer();
                self.escape_armed_at = None;
                return;
            }
            self.rewind_escape();
            return;
        }
        let address = self.active.clone();
        let draft = self.pane().draft.text().to_owned();
        match self.modal {
            Some(Modal::Help) => {}
            Some(Modal::Picker(_)) => self.picker_key(key),
            Some(Modal::Prompt(_)) => self.prompt_key(key),
            Some(Modal::Output(_)) => self.output_key(key),
            Some(Modal::StatusLineEditor(_)) => self.statusline_key(key),
            None => self.composer_key(key),
        }
        if let Some(pane) = self.panes.get_mut(&address) {
            if pane.draft.text() != draft {
                pane.draft_revision = pane.draft_revision.wrapping_add(1);
            }
        }
    }

    pub(super) fn paste(&mut self, text: &str) {
        self.dismiss_confirmation();
        self.text_selection.cancel_drag();
        self.escape_armed_at = None;
        if matches!(&self.modal, Some(Modal::Help)) {
            return;
        }
        let address = self.active.clone();
        let draft = self.pane().draft.text().to_owned();
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            if picker.kind.searchable() {
                picker.query.insert(&text.replace(['\r', '\n'], " "));
                picker.selected = 0;
                self.search(false);
            }
        } else if let Some(Modal::Prompt(prompt)) = &mut self.modal {
            if prompt.kind.multiline() {
                prompt.editor.insert(text);
            } else {
                prompt.editor.insert(&text.replace(['\r', '\n'], " "));
            }
        } else if self.modal.is_none() {
            self.deactivate_context_focus();
            self.focus_composer();
            self.pane_mut().draft.insert(text);
        }
        if let Some(pane) = self.panes.get_mut(&address) {
            if pane.draft.text() != draft {
                pane.draft_revision = pane.draft_revision.wrapping_add(1);
            }
        }
    }
}
