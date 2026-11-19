use kuku_client::{new_id, Action, Command};

use super::{App, PendingItem, PendingStage, RewindDraft};
use crate::jobs::Job;
use crate::transcript::HistoryRequest;

impl App {
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
}
