use kuku_client::{new_id, Action, Command};

use super::{App, PendingItem, PendingStage, RewindDraft};
use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind, RewindHistory};
use crate::text::{shorten, single_line};
use crate::transcript::HistoryRequest;

impl App {
    pub(crate) fn open_rewind_picker(&mut self) {
        let selection = self.selection();
        let next_before = self.pane().transcript.rewind_before();
        let mut messages = self.pane().transcript.rewind_messages(&self.active.agent);
        messages.sort_by_key(|(event_id, _, _)| *event_id);
        let choices = messages
            .into_iter()
            .map(|(event_id, turn, text)| rewind_choice(event_id, turn, text))
            .collect();
        let picker = self.picker(PickerKind::Rewind, choices);
        picker.rewind_history = Some(RewindHistory {
            selection,
            next_before,
            pending_before: None,
        });
        picker.selected = picker.choice_count().saturating_sub(1);
        picker.header = rewind_history_header(next_before, false).into();
    }

    pub(crate) fn rewind_picker_current(&mut self, ticket: u64) -> bool {
        let selection = self.selection();
        let Some(super::Modal::Picker(picker)) = &self.modal else {
            return false;
        };
        if !matches!(picker.kind, PickerKind::Rewind) || picker.ticket.id != ticket {
            return false;
        }
        let current = picker
            .rewind_history
            .as_ref()
            .is_some_and(|history| history.selection == selection);
        if !current {
            self.modal = None;
            self.effects.push(Job::ClosePicker);
            self.problem("Selected history is no longer current · reopen rewind");
        }
        current
    }

    pub(crate) fn move_rewind_selection(&mut self, amount: isize) -> bool {
        let Some(super::Modal::Picker(picker)) = &self.modal else {
            return false;
        };
        if !matches!(picker.kind, PickerKind::Rewind) {
            return false;
        }
        if !self.rewind_picker_current(picker.ticket.id) {
            return true;
        }
        let Some(super::Modal::Picker(picker)) = &mut self.modal else {
            return true;
        };
        picker.move_by(amount);
        if amount < 0 && picker.selected == 0 {
            self.load_rewind_history();
        }
        true
    }

    pub(crate) fn refresh_rewind_history(&mut self) {
        if !self.require_service() {
            return;
        }
        let request = HistoryRequest::Recent;
        let selection = self.selection();
        let pane = self.pane_mut();
        pane.transcript.invalidate_after_rewind();
        if pane.history_in_flight.is_some() {
            pane.history_invalidated = true;
            return;
        }
        pane.history_invalidated = false;
        pane.history_in_flight = Some(request);
        self.effects.push(Job::History(selection, request));
    }

    pub(crate) fn rewind_message(&mut self, event_id: u64, text: String, restore_files: bool) {
        if !self.require_service() {
            return;
        }
        self.pane_mut().draft.remember(&text);
        if self.rewind_drafts.len() >= super::pending::MAX_REWIND_DRAFTS {
            if let Some(oldest) = self.rewind_drafts.keys().next().cloned() {
                self.rewind_drafts.remove(&oldest);
            }
        }
        let address = self.active.clone();
        let command = Command {
            target: self.target(&address),
            command_id: new_id(),
            action: Action::RewindMessage {
                target_event_id: event_id,
                text: text.clone(),
                restore_files,
            },
        };
        let id = command.command_id.clone();
        self.rewind_drafts.insert(
            id.clone(),
            RewindDraft {
                address: address.clone(),
                text,
                revision: self.pane().draft_revision,
            },
        );
        self.pending.insert(
            id.clone(),
            PendingItem {
                command: command.clone(),
                stage: PendingStage::Saving,
                origin: Some(self.generation),
            },
        );
        self.update_pending_delivery(&id);
        self.effects.push(Job::Save(command));
    }

    fn load_rewind_history(&mut self) {
        if !self.require_service() {
            return;
        }
        let Some(super::Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let Some(history) = &mut picker.rewind_history else {
            return;
        };
        if history.pending_before.is_some() {
            return;
        }
        let Some(before) = history.next_before else {
            return;
        };
        history.pending_before = Some(before);
        let selection = history.selection.clone();
        let ticket = picker.begin_search();
        picker.header = rewind_history_header(Some(before), true).into();
        self.effects
            .push(Job::RewindHistory(selection, ticket, before));
    }
}

pub(super) fn rewind_choice(event_id: u64, turn: u64, text: String) -> Choice {
    Choice::new(
        format!("Turn {turn}"),
        shorten(&single_line(&text), 96),
        ChoiceKind::RewindMessage {
            event_id,
            turn,
            text,
        },
    )
}

pub(super) fn rewind_history_header(next_before: Option<u64>, loading: bool) -> &'static str {
    if loading {
        "Loading earlier messages…"
    } else if next_before.is_some() {
        "↑ at the top loads earlier messages"
    } else {
        "All earlier messages loaded"
    }
}
