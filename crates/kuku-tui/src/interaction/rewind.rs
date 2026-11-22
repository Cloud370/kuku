use crossterm::event::KeyEvent;

use crate::app::{App, Modal};
use crate::editor::Editor;
use crate::jobs::Job;
use crate::keys::Shortcut;
use crate::menu::{Prompt, PromptKind};

impl App {
    pub(crate) fn confirm_rewind_message(&mut self, event_id: u64, turn: u64, text: String) {
        self.effects.push(Job::ClosePicker);
        self.modal = Some(Modal::Prompt(Box::new(Prompt {
            kind: PromptKind::RewindMessage {
                event_id,
                text,
                restore_files: false,
            },
            title: format!("Edit past message · Turn {turn}"),
            instructions: String::new(),
            instructions_top: 0,
            editor: Editor::default(),
        })));
    }

    pub(crate) fn escape_interrupts_running(&self) -> bool {
        self.pane().snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.cleaning_up || snapshot.agent.status == kuku_client::AgentStatus::Running
        })
    }

    pub(super) fn rewind_from_menu(&mut self) {
        let pane = self.pane();
        if !pane.exists {
            self.problem("Open a Session before rewinding");
        } else if self.escape_interrupts_running()
            || pane
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.agent.status == kuku_client::AgentStatus::Waiting)
        {
            self.problem("Wait for the Agent to settle before rewinding");
        } else {
            self.open_history_jump();
        }
    }

    pub(super) fn rewind_escape(&mut self) {
        let status = self
            .pane()
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.agent.status);
        if self.escape_interrupts_running() {
            self.rewind_armed = false;
            self.interrupt();
            self.deactivate_context_focus();
            self.focus_composer();
            return;
        }
        if status == Some(kuku_client::AgentStatus::Waiting)
            || self.pane().contextual_focus.active
            || matches!(
                self.pane().keyboard_focus,
                crate::app::KeyboardFocus::Request | crate::app::KeyboardFocus::Queue
            )
        {
            self.rewind_armed = false;
            self.deactivate_context_focus();
            self.focus_composer();
            return;
        }
        if self.pane().keyboard_focus == crate::app::KeyboardFocus::Transcript
            && self.pane().transcript.is_reading()
            && !self.pane().draft.text().is_empty()
        {
            self.rewind_armed = false;
            self.deactivate_context_focus();
            self.focus_composer();
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
