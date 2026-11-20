use std::time::{Duration, Instant};

use crate::app::App;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::text::{shorten, single_line};

pub(super) fn rewind_instructions(text: &str, restore_files: bool) -> String {
    format!(
        "This message and all subsequent events for this Agent will leave active context. The journal remains unchanged.\nRestores only recorded workspace file changes; shell commands and external side effects are not reversed.\n\n{}\n\n[{}] Restore recorded file changes · Space toggles\nEnter commits · Esc cancels",
        text,
        if restore_files { "x" } else { " " }
    )
}

impl App {
    pub(crate) fn escape_interrupts_running(&self) -> bool {
        self.pane().snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.cleaning_up || snapshot.agent.status == kuku_client::AgentStatus::Running
        })
    }

    pub(super) fn rewind_escape(&mut self) {
        let status = self
            .pane()
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.agent.status);
        if self.escape_interrupts_running() {
            self.escape_armed_at = None;
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
            self.escape_armed_at = None;
            self.deactivate_context_focus();
            self.focus_composer();
            return;
        }
        if !self.pane().draft.text().is_empty() {
            self.escape_armed_at = None;
            return;
        }
        let now = Instant::now();
        if self
            .escape_armed_at
            .is_some_and(|armed| now.duration_since(armed) <= Duration::from_millis(500))
        {
            self.escape_armed_at = None;
            self.open_rewind_picker();
        } else {
            self.escape_armed_at = Some(now);
            self.message("Press Esc again within 500 ms to rewind to a message");
        }
    }

    fn open_rewind_picker(&mut self) {
        let messages = self.pane().transcript.rewind_messages(&self.active.agent);
        let choices = messages
            .into_iter()
            .map(|(event_id, turn, text)| {
                Choice::new(
                    format!("Turn {turn}"),
                    shorten(&single_line(&text), 96),
                    ChoiceKind::RewindMessage {
                        event_id,
                        turn,
                        text,
                    },
                )
            })
            .collect();
        let picker = self.picker(PickerKind::Rewind, choices);
        picker.header = "Loaded history only · older messages are not loaded".into();
    }
}
