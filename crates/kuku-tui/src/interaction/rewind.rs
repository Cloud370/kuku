use std::time::{Duration, Instant};

use super::*;

pub(super) fn rewind_instructions(text: &str, restore_files: bool) -> String {
    format!(
        "This message and all subsequent events for this Agent will leave active context. The journal remains unchanged.\n\n{}\n\n[{}] Restore recorded file changes · Space toggles\nEnter commits · Esc cancels",
        text,
        if restore_files { "x" } else { " " }
    )
}

impl App {
    pub(super) fn rewind_escape(&mut self) {
        let draft_empty = self.pane().draft.text().is_empty();
        let busy = self.pane().execution().is_some()
            || self.pane().snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.cleaning_up
                    || matches!(
                        snapshot.agent.status,
                        kuku_client::AgentStatus::Running | kuku_client::AgentStatus::Waiting
                    )
                    || !snapshot.pending.is_empty()
            });
        if !draft_empty || busy {
            self.escape_armed_at = None;
            self.interrupt();
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
