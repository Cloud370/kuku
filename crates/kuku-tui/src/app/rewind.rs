use kuku_client::{Action, Command, Error, MessageDraft, new_id};

use super::{App, PendingItem, PendingStage, RewindDraft};
use crate::jobs::{Job, Selection};
use crate::menu::{Choice, ChoiceKind, PickerKind, RewindHistory, RewindLoad, Ticket};
use crate::text::{shorten, single_line};
use crate::transcript::{HistoryRequest, MAX_BODY_BYTES, MAX_RECORDS};

pub(crate) const REWIND_PAGE: usize = 12;
const PREFETCH_UP_ROWS: usize = 2;

pub(super) enum RewindWindow {
    Recent,
    Earlier,
}

impl App {
    pub(crate) fn open_rewind_picker(&mut self) {
        let selection = self.selection();
        let mut next_before = self.pane().transcript.rewind_before();
        let mut messages = self.pane().transcript.rewind_messages(&self.active.agent);
        messages.sort_by_key(|(event_id, _, _)| *event_id);
        let selected = messages.last().map(|(event_id, _, _)| *event_id);
        let mut choices = messages
            .into_iter()
            .map(|(event_id, turn, text)| rewind_choice(event_id, turn, text))
            .collect();
        let window_limited = bound_rewind_choices(&mut choices, selected, RewindWindow::Recent);
        if window_limited {
            next_before = choices.first().and_then(rewind_event_id);
        }
        let preload = self.pane().exists && choices.len() < REWIND_PAGE && next_before.is_some();
        let picker = self.picker(PickerKind::Rewind, choices);
        picker.rewind_history = Some(RewindHistory {
            selection,
            next_before,
            load: RewindLoad::Idle,
            window_limited,
        });
        picker.selected = picker.choice_count().saturating_sub(1);
        picker.header = rewind_history_header(&picker.kind, next_before, false, window_limited);
        if preload {
            self.load_rewind_history(0);
        }
    }

    pub(crate) fn rewind_picker_current(&mut self, ticket: u64) -> bool {
        let selection = self.selection();
        let Some(super::Modal::Picker(picker)) = &self.modal else {
            return false;
        };
        if !matches!(picker.kind, PickerKind::Rewind | PickerKind::HistoryJump)
            || picker.ticket.id != ticket
        {
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
        if !matches!(picker.kind, PickerKind::Rewind | PickerKind::HistoryJump) {
            return false;
        }
        if !self.rewind_picker_current(picker.ticket.id) {
            return true;
        }
        let Some(super::Modal::Picker(picker)) = &mut self.modal else {
            return true;
        };
        if picker
            .rewind_history
            .as_ref()
            .is_some_and(|history| matches!(history.load, RewindLoad::Source { .. }))
        {
            return true;
        }
        let pending_up = amount
            .unsigned_abs()
            .saturating_sub(picker.selected)
            .min(REWIND_PAGE);
        if amount > 0 {
            if let Some(RewindHistory {
                load: RewindLoad::Page { pending_up, .. },
                ..
            }) = &mut picker.rewind_history
            {
                *pending_up = 0;
            }
        }
        picker.move_by(amount);
        if amount < 0
            && (picker.selected == 0
                || picker.selected <= PREFETCH_UP_ROWS && picker.problem.is_none())
        {
            self.load_rewind_history(pending_up);
        }
        true
    }

    pub(crate) fn load_rewind_source(&mut self, ticket: u64, event_id: u64, turn: u64) {
        if !self.rewind_picker_current(ticket) || !self.require_service() {
            return;
        }
        let Some(super::Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let Some(history) = &mut picker.rewind_history else {
            return;
        };
        let source = RewindLoad::Source { event_id, turn };
        if history.load == source {
            return;
        }
        history.load = source;
        let selection = history.selection.clone();
        let selected = picker.selected;
        let ticket = picker.begin_search();
        picker.selected = selected;
        picker.header = "Loading original message… · Esc cancels".into();
        self.effects
            .push(Job::RewindSource(selection, ticket, event_id));
    }

    pub(super) fn apply_rewind_source_update(
        &mut self,
        selection: Selection,
        ticket: Ticket,
        event_id: u64,
        result: Result<MessageDraft, Error>,
    ) {
        if selection != self.selection() {
            return;
        }
        let Some(super::Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        if picker.ticket != ticket {
            return;
        }
        let Some(history) = &mut picker.rewind_history else {
            return;
        };
        let RewindLoad::Source {
            event_id: expected,
            turn,
        } = history.load
        else {
            return;
        };
        if history.selection != selection || expected != event_id {
            return;
        }
        history.load = RewindLoad::Idle;
        picker.loading = false;
        match result {
            Ok(draft) => self.confirm_rewind_message(event_id, turn, draft),
            Err(error) => {
                picker.header = rewind_history_header(
                    &picker.kind,
                    history.next_before,
                    false,
                    history.window_limited,
                );
                picker.problem = Some(format!("Message · {error} · Enter retries"));
            }
        }
    }

    pub(crate) fn refresh_rewind_history(&mut self, commit: u64) {
        if !self.require_service() {
            return;
        }
        let request = HistoryRequest::Recent;
        let selection = self.selection();
        let pane = self.pane_mut();
        if commit <= pane.last_history_change {
            return;
        }
        pane.last_history_change = commit;
        pane.transcript.invalidate_after_rewind();
        if pane.history_in_flight.is_some() {
            pane.history_invalidated = true;
            return;
        }
        pane.history_invalidated = false;
        pane.history_in_flight = Some(request);
        self.effects.push(Job::History(selection, request));
    }

    pub(super) fn restore_rewind_draft(&mut self, draft: &mut RewindDraft) -> bool {
        *draft.restored.get_or_insert_with(|| {
            let Some(pane) = self.panes.get_mut(&draft.address) else {
                return false;
            };
            if pane.draft_revision != draft.revision
                || !pane.draft.text().is_empty()
                || !pane.value_attachments.is_empty()
            {
                return false;
            }
            pane.restore_draft_images(&draft.draft.text, draft.draft.images.clone());
            if self.active == draft.address {
                self.deactivate_context_focus();
                self.focus_composer();
            }
            true
        })
    }

    pub(crate) fn rewind_message(
        &mut self,
        event_id: u64,
        draft: MessageDraft,
        restore_files: bool,
    ) {
        if !self.pane().draft.text().is_empty() || !self.pane().value_attachments.is_empty() {
            self.problem("Clear the draft before editing a past message");
            return;
        }
        self.issue_history_change(
            Action::RewindMessage {
                target_event_id: event_id,
                text: draft.text.clone(),
                draft_images: draft.images.clone(),
                restore_files,
            },
            Some(draft),
        );
    }

    pub(super) fn issue_history_change(&mut self, action: Action, draft: Option<MessageDraft>) {
        if !self.require_service() {
            return;
        }
        let address = self.active.clone();
        let command = Command {
            target: self.target(&address),
            command_id: new_id(),
            action,
        };
        let id = command.command_id.clone();
        if let Some(draft) = draft {
            self.pane_mut().draft.remember_draft(draft.clone());
            if self.rewind_drafts.len() >= super::pending::MAX_REWIND_DRAFTS {
                if let Some(oldest) = self.rewind_drafts.keys().next().cloned() {
                    self.rewind_drafts.remove(&oldest);
                }
            }
            self.rewind_drafts.insert(
                id.clone(),
                RewindDraft {
                    address,
                    draft,
                    revision: self.pane().draft_revision,
                    restored: None,
                },
            );
        }
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

    fn load_rewind_history(&mut self, pending_up: usize) {
        if !self.require_service() {
            return;
        }
        let Some(super::Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let Some(history) = &mut picker.rewind_history else {
            return;
        };
        match &mut history.load {
            RewindLoad::Page {
                pending_up: queued, ..
            } => {
                *queued = (*queued).max(pending_up);
                return;
            }
            RewindLoad::Source { .. } => return,
            RewindLoad::Idle => {}
        }
        let Some(before) = history.next_before else {
            return;
        };
        history.load = RewindLoad::Page {
            before: Some(before),
            pending_up,
        };
        let selection = history.selection.clone();
        let window_limited = history.window_limited;
        let selected = picker.selected;
        let ticket = picker.begin_search();
        picker.selected = selected;
        picker.header = rewind_history_header(&picker.kind, Some(before), true, window_limited);
        let job = if picker.kind == PickerKind::HistoryJump {
            Job::HistoryJump(selection, ticket, Some(before))
        } else {
            Job::RewindHistory(selection, ticket, before)
        };
        self.effects.push(job);
    }
}

pub(super) fn rewind_choice(event_id: u64, turn: u64, text: String) -> Choice {
    Choice::new(
        format!("Turn {turn}"),
        shorten(&single_line(&text), 160),
        ChoiceKind::RewindMessage {
            event_id,
            turn,
            text,
        },
    )
}

pub(super) fn rewind_history_header(
    kind: &PickerKind,
    next_before: Option<u64>,
    loading: bool,
    window_limited: bool,
) -> String {
    let items = if *kind == PickerKind::HistoryJump {
        "turns"
    } else {
        "messages"
    };
    let status = if loading {
        format!("Loading earlier {items}…")
    } else if next_before.is_some() {
        format!("↑ at the top loads earlier {items}")
    } else {
        format!("All earlier {items} loaded")
    };
    if window_limited {
        format!("{status}\nHistory window limit reached · reopen rewind for recent {items}")
    } else {
        status
    }
}

pub(super) fn bound_rewind_choices(
    choices: &mut Vec<Choice>,
    selected: Option<u64>,
    window: RewindWindow,
) -> bool {
    let mut bytes: usize = choices.iter().map(rewind_text_bytes).sum();
    let mut limited = false;
    while choices.len() > MAX_RECORDS || bytes > MAX_RECORDS * MAX_BODY_BYTES {
        let removable = |choice: &Choice| rewind_event_id(choice) != selected;
        let index = match window {
            RewindWindow::Recent => choices.iter().position(removable),
            RewindWindow::Earlier => choices.iter().rposition(removable),
        }
        // NOTE: Opening choices use bounded transcript bodies; later selections come from this already bounded window.
        .expect("the selected message fits the rewind window");
        bytes -= rewind_text_bytes(&choices.remove(index));
        limited = true;
    }
    limited
}

pub(super) fn rewind_event_id(choice: &Choice) -> Option<u64> {
    match &choice.kind {
        ChoiceKind::RewindMessage { event_id, .. } => Some(*event_id),
        ChoiceKind::HistoryJumpReview(destination) => Some(destination.point.event_id),
        _ => None,
    }
}

fn rewind_text_bytes(choice: &Choice) -> usize {
    match &choice.kind {
        ChoiceKind::RewindMessage { text, .. } => text.len(),
        _ => 0,
    }
}
