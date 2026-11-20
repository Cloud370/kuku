use crate::app::App;
use crate::jobs::{Job, Selection};
use crate::transcript::{RewindGroupAction, RewindGroupRequest};

impl App {
    pub(crate) fn toggle_readonly_rewind_group(&mut self, rewind_event_id: u64) -> bool {
        self.dispatch_readonly_rewind_group(rewind_event_id, RewindGroupAction::Toggle)
    }

    pub(crate) fn load_more_readonly_rewind_group(&mut self, rewind_event_id: u64) -> bool {
        self.dispatch_readonly_rewind_group(rewind_event_id, RewindGroupAction::More)
    }

    pub(crate) fn retry_readonly_rewind_group(&mut self, rewind_event_id: u64) -> bool {
        self.dispatch_readonly_rewind_group(rewind_event_id, RewindGroupAction::Retry)
    }

    pub(crate) fn activate_readonly_rewind_group(
        &mut self,
        rewind_event_id: u64,
        action: RewindGroupAction,
    ) -> bool {
        match action {
            RewindGroupAction::Toggle => self.toggle_readonly_rewind_group(rewind_event_id),
            RewindGroupAction::More => self.load_more_readonly_rewind_group(rewind_event_id),
            RewindGroupAction::Retry => self.retry_readonly_rewind_group(rewind_event_id),
        }
    }

    pub(crate) fn activate_selected_readonly_rewind_group(
        &mut self,
        selection: Selection,
        rewind_event_id: u64,
        action: RewindGroupAction,
    ) -> bool {
        if selection != self.selection() {
            self.problem("Selected rewind group is no longer available");
            return false;
        }
        let available = self
            .pane()
            .transcript
            .rewind_group_choices()
            .into_iter()
            .find(|choice| choice.rewind_event_id == rewind_event_id)
            .is_some_and(|choice| match action {
                RewindGroupAction::Toggle => true,
                RewindGroupAction::More => choice.can_load_more,
                RewindGroupAction::Retry => choice.can_retry,
            });
        if !available {
            self.problem("Selected rewind group action is no longer available");
            return false;
        }
        self.activate_readonly_rewind_group(rewind_event_id, action)
    }

    fn dispatch_readonly_rewind_group(
        &mut self,
        rewind_event_id: u64,
        action: RewindGroupAction,
    ) -> bool {
        let available = self
            .pane()
            .transcript
            .rewind_group_choices()
            .iter()
            .any(|choice| choice.rewind_event_id == rewind_event_id);
        let request = match action {
            RewindGroupAction::Toggle => self
                .pane_mut()
                .transcript
                .toggle_rewind_group(rewind_event_id),
            RewindGroupAction::More => self
                .pane_mut()
                .transcript
                .load_more_rewind_group(rewind_event_id),
            RewindGroupAction::Retry => self
                .pane_mut()
                .transcript
                .retry_rewind_group(rewind_event_id),
        };
        let expanded = action == RewindGroupAction::Toggle
            && self.pane().transcript.is_reading()
            && self
                .pane()
                .transcript
                .rewind_group_choices()
                .iter()
                .any(|choice| choice.rewind_event_id == rewind_event_id && choice.expanded);
        if expanded {
            self.pane_mut()
                .transcript
                .set_focused_rewind_group(Some(rewind_event_id));
            self.focus_history();
        } else if action == RewindGroupAction::Toggle && available {
            self.pane_mut().transcript.set_focused_rewind_group(None);
            self.focus_composer();
        }
        let Some(request) = request else {
            return action == RewindGroupAction::Toggle && available;
        };
        self.request_rewind_group_page(request);
        true
    }

    fn request_rewind_group_page(&mut self, request: RewindGroupRequest) {
        self.effects
            .push(Job::RewindGroupHistory(self.selection(), request));
    }
}
