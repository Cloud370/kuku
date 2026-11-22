use kuku_client::{
    Action, AgentStatus, Error, FileRewindPreview, HistoryDestination, HistoryDestinations,
};

use super::{App, Modal};
use crate::jobs::{Job, Selection};
use crate::menu::{
    Choice, ChoiceKind, Picker, PickerKind, PromptKind, RewindHistory, RewindLoad, Ticket,
};

impl App {
    pub(crate) fn open_history_jump(&mut self) {
        let pane = self.pane();
        if !pane.exists {
            self.problem("Open a Session before switching conversation history");
            return;
        }
        if pane.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.cleaning_up
                || snapshot.agent.pending_inputs > 0
                || matches!(
                    snapshot.agent.status,
                    AgentStatus::Running | AgentStatus::Waiting
                )
        }) {
            self.problem("Wait for the Agent and its pending input to settle before switching");
            return;
        }
        if !self.require_service() {
            self.open_rewind_picker();
            return;
        }
        let selection = self.selection();
        let picker = self.picker(PickerKind::HistoryJump, Vec::new());
        picker.rewind_history = Some(RewindHistory {
            selection: selection.clone(),
            next_before: None,
            load: RewindLoad::Page {
                before: None,
                pending_up: 0,
            },
            window_limited: false,
        });
        picker.header = "Loading recent turns…".into();
        let ticket = picker.begin_search();
        self.effects.push(Job::HistoryJump(selection, ticket, None));
    }

    pub(super) fn apply_history_jump(
        &mut self,
        selection: Selection,
        ticket: Ticket,
        before: Option<u64>,
        result: Result<HistoryDestinations, Error>,
    ) {
        let choices = result.map(|page| {
            let choices = page
                .points
                .into_iter()
                .filter(|destination| {
                    before.is_none_or(|cursor| destination.point.event_id < cursor)
                })
                .map(|destination| {
                    let preview = destination
                        .point
                        .preview
                        .as_deref()
                        .map_or_else(String::new, crate::text::single_line);
                    Choice::new(
                        format!("Turn {}", destination.point.number),
                        preview,
                        ChoiceKind::HistoryJumpReview(destination),
                    )
                })
                .collect();
            (choices, page.next_before)
        });
        self.apply_rewind_page(selection, ticket, before, choices);
    }

    pub(crate) fn review_history_jump(&mut self, destination: HistoryDestination) {
        if matches!(&self.modal, Some(Modal::Picker(picker)) if picker.loading) {
            return;
        }
        let selection = self.selection();
        let ticket = self.modal_id();
        let existing_draft =
            !self.pane().draft.text().is_empty() || !self.pane().value_attachments.is_empty();
        let load_draft = destination
            .prompt_event_id
            .filter(|_| !existing_draft || !destination.point.can_continue);
        let title = format!("Switch to Turn {}", destination.point.number);
        let previous = match self.modal.take() {
            Some(Modal::Picker(picker)) => Some(picker),
            _ => None,
        };
        self.modal_selection.clear();
        self.effects.push(Job::ClosePicker);
        self.prompt(
            PromptKind::HistoryJump(Box::new(HistoryJumpPrompt {
                selection: selection.clone(),
                destination,
                previous,
                existing_draft,
                draft: load_draft.map_or(Preview::Ready(None), |_| Preview::Loading(ticket)),
                files: None,
            })),
            title,
            "",
        );
        if let Some(event_id) = load_draft {
            self.effects
                .push(Job::HistoryJumpDraft(selection, ticket, event_id));
        }
    }

    pub(crate) fn back_from_history_jump(&mut self) -> bool {
        let Some(prompt) = self.history_jump_prompt() else {
            return false;
        };
        self.modal = prompt.previous.take().map(Modal::Picker);
        self.modal_selection.clear();
        true
    }

    pub(crate) fn toggle_history_jump_files(&mut self) {
        let request = self.modal_id();
        let Some(prompt) = self.history_jump_prompt() else {
            return;
        };
        if !prompt.can_restore_files() {
            return;
        }
        if prompt.file_restore_selected() {
            prompt.files = None;
        } else {
            prompt.files = Some(Preview::Loading(request));
            let job = Job::FileRewindPreview(
                prompt.selection.clone(),
                request,
                prompt.destination.point.event_id,
            );
            self.effects.push(job);
        }
        if let Some(Modal::Prompt(prompt)) = &mut self.modal {
            prompt.instructions_top = 0;
        }
    }

    pub(super) fn history_jump_draft(
        &mut self,
        selection: Selection,
        ticket: u64,
        result: Result<String, Error>,
    ) {
        if selection == self.selection() {
            if let Some(prompt) = self.history_jump_prompt() {
                prompt.draft.complete(ticket, result.map(Some));
            }
        }
    }

    pub(super) fn history_jump_files(
        &mut self,
        selection: Selection,
        ticket: u64,
        result: Result<FileRewindPreview, Error>,
    ) {
        if selection == self.selection() {
            if let Some(files) = self
                .history_jump_prompt()
                .and_then(|prompt| prompt.files.as_mut())
            {
                files.complete(ticket, result);
            }
        }
    }

    pub(crate) fn confirm_history_jump(&mut self) {
        let selection = self.selection();
        let Some(prompt) = self.history_jump_prompt() else {
            return;
        };
        if prompt.selection != selection {
            self.modal = None;
            return;
        }
        if !prompt.ready() {
            if matches!(prompt.draft, Preview::Failed(_)) {
                let event_id = prompt
                    .destination
                    .prompt_event_id
                    .expect("draft request has a source");
                let ticket = self.modal_id();
                if let Some(prompt) = self.history_jump_prompt() {
                    prompt.draft = Preview::Loading(ticket);
                }
                self.effects
                    .push(Job::HistoryJumpDraft(selection, ticket, event_id));
            }
            return;
        }
        let Some(Modal::Prompt(prompt)) = self.modal.take() else {
            unreachable!("checked history confirmation");
        };
        let PromptKind::HistoryJump(prompt) = prompt.kind else {
            unreachable!("checked history confirmation");
        };
        let Preview::Ready(draft) = prompt.draft else {
            unreachable!("checked draft readiness");
        };
        let action = if prompt.destination.point.can_continue {
            Action::HistoryJump {
                target_event_id: prompt.destination.point.event_id,
                file_restore: prompt.files.and_then(|files| {
                    let Preview::Ready(files) = files else {
                        unreachable!("checked file preview readiness");
                    };
                    (!files.changes.is_empty()).then_some(files.fingerprint)
                }),
            }
        } else {
            Action::RewindMessage {
                target_event_id: prompt
                    .destination
                    .prompt_event_id
                    .expect("unfinished turn has a source"),
                text: draft.clone().expect("unfinished turn has a message"),
                restore_files: false,
            }
        };
        self.issue_history_change(action, draft.filter(|_| !prompt.existing_draft));
    }

    fn history_jump_prompt(&mut self) -> Option<&mut HistoryJumpPrompt> {
        let Some(Modal::Prompt(prompt)) = &mut self.modal else {
            return None;
        };
        match &mut prompt.kind {
            PromptKind::HistoryJump(review) => Some(review),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Preview<T> {
    Loading(u64),
    Ready(T),
    Failed(String),
}

impl<T> Preview<T> {
    fn complete(&mut self, ticket: u64, result: Result<T, Error>) {
        if matches!(self, Self::Loading(current) if *current == ticket) {
            *self = match result {
                Ok(value) => Self::Ready(value),
                Err(error) => Self::Failed(error.to_string()),
            };
        }
    }
}

#[derive(Debug)]
pub(crate) struct HistoryJumpPrompt {
    selection: Selection,
    previous: Option<Box<Picker>>,
    pub(crate) destination: HistoryDestination,
    pub(crate) existing_draft: bool,
    pub(crate) draft: Preview<Option<String>>,
    pub(crate) files: Option<Preview<FileRewindPreview>>,
}

impl HistoryJumpPrompt {
    pub(crate) fn can_restore_files(&self) -> bool {
        self.destination.current_branch && self.destination.point.can_continue
    }

    pub(crate) fn file_restore_selected(&self) -> bool {
        self.files
            .as_ref()
            .is_some_and(|files| !matches!(files, Preview::Ready(plan) if plan.changes.is_empty()))
    }

    pub(crate) fn ready(&self) -> bool {
        matches!(self.draft, Preview::Ready(_))
            && self
                .files
                .as_ref()
                .is_none_or(|files| matches!(files, Preview::Ready(_)))
    }
}
