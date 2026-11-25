use crossterm::event::KeyEvent;

use crate::app::{App, Modal};
use crate::editor::Editor;
use crate::jobs::Job;
use crate::keys::Shortcut;
use crate::menu::{Prompt, PromptKind};

impl App {
    pub(crate) fn confirm_rewind_message(
        &mut self,
        event_id: u64,
        turn: u64,
        draft: kuku_client::MessageDraft,
    ) {
        self.effects.push(Job::ClosePicker);
        self.modal = Some(Modal::Prompt(Box::new(Prompt {
            kind: PromptKind::RewindMessage {
                event_id,
                text: draft.text,
                images: draft.images,
                restore_files: false,
            },
            title: format!("Edit past message · Turn {turn}"),
            instructions: String::new(),
            instructions_top: 0,
            editor: Editor::default(),
        })));
    }

    pub(super) fn rewind_from_menu(&mut self) {
        let pane = self.pane();
        if !pane.exists {
            self.problem("Open a Session before rewinding");
        } else if pane.busy_or_waiting() {
            self.problem("Wait for the Agent to settle before rewinding");
        } else {
            self.open_history_jump();
        }
    }

    pub(super) fn rewind_escape(&mut self) {
        if !self.composer_focused() {
            self.rewind_armed = false;
            self.deactivate_context_focus();
            self.focus_composer();
            return;
        }
        if self.pane().running_execution().is_some() {
            self.rewind_armed = false;
            self.interrupt_current_turn();
            self.deactivate_context_focus();
            self.focus_composer();
            return;
        }
        if self.pane().busy_or_waiting() {
            self.rewind_armed = false;
            return;
        }
        if !self.pane().draft.text().is_empty() {
            self.rewind_armed = false;
            return;
        }
        if self.rewind_armed {
            self.rewind_armed = false;
            self.rewind_from_menu();
        } else {
            self.rewind_armed = true;
            self.confirmation("Press Esc again to open conversation history");
        }
    }

    pub(super) fn rewind_picker_key(&mut self, key: KeyEvent) -> bool {
        let amount = if Shortcut::UP.matches(key) {
            -1
        } else if Shortcut::DOWN.matches(key) {
            1
        } else if Shortcut::PAGE_UP.matches(key) {
            -(self.modal_page.max(1) as isize)
        } else if Shortcut::PAGE_DOWN.matches(key) {
            self.modal_page.max(1) as isize
        } else {
            return false;
        };
        self.move_rewind_selection(amount)
    }
}
