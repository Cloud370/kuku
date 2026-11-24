use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::{App, Job, Modal, PromptKind, Shortcut};
use crate::menu::{ChoiceKind, PickerKind};

impl App {
    pub(super) fn key(&mut self, key: KeyEvent) {
        self.dismiss_confirmation();
        if Shortcut::PERMISSIONS.matches(key) && key.kind == KeyEventKind::Press {
            self.open_permissions();
            return;
        }
        if self.modal.is_none()
            && Shortcut::REPLY.matches(key)
            && key.kind == KeyEventKind::Press
            && self.open_approvals()
        {
            return;
        }
        if matches!(self.modal, Some(Modal::SendRouteConfirmation(_)))
            && !Shortcut::ESCAPE.matches(key)
            && !Shortcut::ENTER.matches(key)
        {
            self.close_picker();
        }
        if self.modal.is_some() {
            self.modal_selection.cancel_drag();
        } else if !(self.opens_command_menu(key)
            || Shortcut::ACTIONS.matches(key)
            || Shortcut::ACTIONS_ALIAS.matches(key)
            || Shortcut::CYCLE_THINKING.matches(key)
            || (Shortcut::ENTER.matches(key) || Shortcut::NEXT_TURN.matches(key))
                && self.bare_exit_draft())
        {
            self.text_selection.cancel_drag();
        }
        if !Shortcut::ESCAPE.matches(key) {
            self.rewind_armed = false;
        }
        if Shortcut::ESCAPE.matches(key) {
            if let Some(Modal::Picker(picker)) = &mut self.modal {
                if key.kind == KeyEventKind::Press
                    && picker.kind == PickerKind::Models
                    && picker.search_focused
                {
                    picker.search_focused = false;
                    return;
                }
            }
            if key.kind == KeyEventKind::Repeat
                || self.back_from_thinking_confirmation()
                || self.back_from_agent_setup()
                || self.back_from_history_jump()
            {
                self.rewind_armed = false;
                return;
            }
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
                self.rewind_armed = false;
                return;
            }
            if self.modal.is_none() && self.pane().transcript.focused_rewind_group().is_some() {
                self.deactivate_context_focus();
                self.focus_composer();
                self.rewind_armed = false;
                return;
            }
            let messages_picker = matches!(
                &self.modal,
                Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Messages)
            );
            if let Some(modal) = self.modal.take() {
                self.modal_selection.clear();
                let close_picker = !matches!(&modal, Modal::Help | Modal::ExitConfirmation);
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
                self.rewind_armed = false;
                return;
            }
            if self.pane_mut().transcript.close_notes() {
                self.rewind_armed = false;
                return;
            }
            self.rewind_escape();
            return;
        }
        let address = self.active.clone();
        let draft = self.pane().draft.text().to_owned();
        match self.modal {
            Some(Modal::Help) if key.code == KeyCode::Char('/') && super::is_text(key) => {
                self.command_menu()
            }
            Some(Modal::Help) => {}
            Some(Modal::Picker(_)) => self.picker_key(key),
            Some(Modal::Prompt(_)) => self.prompt_key(key),
            Some(Modal::Output(_)) => self.output_key(key),
            Some(Modal::StatusLineEditor(_)) => self.statusline_key(key),
            Some(Modal::Notifications(_)) => self.notification_key(key),
            Some(Modal::Permissions(_)) => self.permission_mode_key(key),
            Some(Modal::Approvals(_)) => self.approval_key(key),
            Some(Modal::ExitConfirmation) => self.exit_confirmation_key(key),
            Some(Modal::SendRouteConfirmation(_)) => {
                if Shortcut::ENTER.matches(key) && key.kind == KeyEventKind::Press {
                    self.confirm_send_route();
                }
            }

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
        if matches!(self.modal, Some(Modal::SendRouteConfirmation(_))) {
            self.close_picker();
        }
        if self.modal.is_some() {
            self.modal_selection.cancel_drag();
        } else {
            self.text_selection.cancel_drag();
        }
        self.rewind_armed = false;
        if matches!(&self.modal, Some(Modal::Help)) {
            return;
        }
        let address = self.active.clone();
        let draft = self.pane().draft.text().to_owned();
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            if picker.kind.searchable()
                && (picker.kind != PickerKind::Models || picker.search_focused)
            {
                picker.query.insert(&text.replace(['\r', '\n'], " "));
                self.picker_query_changed();
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
