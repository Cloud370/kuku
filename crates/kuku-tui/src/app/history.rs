use kuku_client::{Error, HistoryPage, RewindGroupPage};

use crate::jobs::{Job, Selection};
use crate::transcript::{HistoryRequest, RewindGroupRequest};

use super::App;

impl App {
    pub(super) fn apply_history_update(
        &mut self,
        selection: Selection,
        request: HistoryRequest,
        result: Result<HistoryPage, Error>,
    ) {
        if selection != self.selection() {
            return;
        }
        if self.pane().history_invalidated {
            let request = HistoryRequest::Recent;
            let pane = self.pane_mut();
            pane.history_invalidated = false;
            pane.history_in_flight = Some(request);
            self.effects.push(Job::History(selection, request));
        } else {
            self.pane_mut().history_in_flight = None;
            match result {
                Ok(page) => self.pane_mut().transcript.apply_page(request, page),
                Err(error) => self.problem(format!("History · {error} · scroll up to retry")),
            }
        }
    }

    pub(super) fn apply_rewind_group_history_update(
        &mut self,
        selection: Selection,
        request: RewindGroupRequest,
        result: Result<RewindGroupPage, Error>,
    ) {
        if selection != self.selection() {
            return;
        }
        let result = result.map_err(|error| error.to_string());
        self.pane_mut()
            .transcript
            .apply_rewind_group_page(&request, result);
    }
}
