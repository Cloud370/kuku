use crossterm::event::KeyEvent;

use crate::app::{App, Modal};
use crate::editor::Editor;
use crate::jobs::Job;
use crate::keys::Shortcut;
use crate::menu::{Prompt, PromptKind};

pub(super) fn rewind_instructions(text: &str, restore_files: bool) -> String {
    format!(
        "This message and all subsequent events for this Agent will leave active context. The journal remains unchanged.\nRestores only recorded workspace file changes; shell commands and external side effects are not reversed.\n\n{}\n\n[{}] Restore recorded file changes · Space toggles\nEnter commits · Esc cancels",
        text,
        if restore_files { "x" } else { " " }
    )
}

impl App {
    pub(crate) fn confirm_rewind_message(&mut self, event_id: u64, turn: u64, text: String) {
        self.effects.push(Job::ClosePicker);
        self.rewind_restore_files = false;
        self.modal = Some(Modal::Prompt(Box::new(Prompt {
            kind: PromptKind::RewindMessage {
                event_id,
                text: text.clone(),
            },
            title: format!("Rewind to message · Turn {turn}"),
            instructions: rewind_instructions(&text, false),
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
        } else if !pane.draft.text().is_empty() || !pane.value_attachments.is_empty() {
            self.problem("Clear the draft before rewinding");
        } else {
            self.open_rewind_picker();
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
            self.open_rewind_picker();
        } else {
            self.rewind_armed = true;
            self.confirmation("Press Esc again to rewind to a message");
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
