use crate::app::App;
use crate::jobs::Job;
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
            self.focus_history();
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
