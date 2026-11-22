use std::collections::BTreeMap;

use kuku_client::{Error, HistoryPage, RewindGroupPage};

use crate::jobs::{Job, Selection};
use crate::menu::{Choice, ChoiceKind, Picker, RewindLoad, Ticket};
use crate::transcript::{HistoryRequest, RewindGroupRequest};

use super::rewind::{
    bound_rewind_choices, rewind_choice, rewind_event_id, rewind_history_header, RewindWindow,
};
use super::App;

impl App {
    pub(super) fn apply_rewind_history_update(
        &mut self,
        selection: Selection,
        ticket: Ticket,
        before: u64,
        result: Result<HistoryPage, Error>,
    ) {
        let choices = result.map(|page| {
            let agent = &selection.target.agent;
            let choices = page
                .entries
                .into_iter()
                .filter(|entry| {
                    entry.id < before
                        && &entry.agent == agent
                        && entry.kind == kuku_client::HistoryKind::User
                })
                .filter_map(|entry| {
                    let turn = entry
                        .details
                        .get("turn_number")
                        .and_then(serde_json::Value::as_u64)
                        .or(entry.turn)?;
                    Some(rewind_choice(
                        entry.id,
                        turn,
                        entry.text.unwrap_or_default(),
                    ))
                })
                .collect();
            (choices, page.next_before)
        });
        self.apply_rewind_page(selection, ticket, Some(before), choices);
    }

    pub(super) fn apply_rewind_page(
        &mut self,
        selection: Selection,
        ticket: Ticket,
        before: Option<u64>,
        result: Result<(Vec<Choice>, Option<u64>), Error>,
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
        let RewindLoad::Page {
            before: expected,
            pending_up,
        } = history.load
        else {
            return;
        };
        if history.selection != selection || expected != before {
            return;
        }
        history.load = RewindLoad::Idle;
        picker.loading = false;
        match result {
            Ok((choices, next_before))
                if before.is_none_or(|cursor| next_before.is_none_or(|next| next < cursor)) =>
            {
                history.next_before = next_before;
                picker.problem = None;
                let limited = merge_rewind_choices(picker, choices, pending_up);
                let history = picker
                    .rewind_history
                    .as_mut()
                    .expect("rewind history exists");
                history.window_limited |= limited;
                picker.header =
                    rewind_history_header(&picker.kind, next_before, false, history.window_limited);
            }
            Ok(_) => {
                picker.problem = Some("History did not advance · ↑ at the top retries".into());
                picker.header = rewind_history_header(
                    &picker.kind,
                    history.next_before,
                    false,
                    history.window_limited,
                );
            }
            Err(error) => {
                picker.header = rewind_history_header(
                    &picker.kind,
                    history.next_before,
                    false,
                    history.window_limited,
                );
                if before.is_none() {
                    picker.set_local(vec![Choice::new(
                        "Retry",
                        "",
                        ChoiceKind::HistoryJumpLatest,
                    )]);
                    picker.problem = Some(format!("History · {error} · Enter retries"));
                } else {
                    picker.problem = Some(format!("History · {error} · ↑ at the top retries"));
                }
            }
        }
    }

    pub(super) fn apply_history_update(
        &mut self,
        selection: Selection,
        request: HistoryRequest,
        result: Result<crate::jobs::LoadedHistory, Error>,
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
                Ok(crate::jobs::LoadedHistory::Before(page)) => {
                    self.pane_mut().transcript.apply_page(request, page)
                }
                Ok(crate::jobs::LoadedHistory::After(page)) => {
                    self.pane_mut().transcript.apply_forward_page(request, page)
                }
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

fn merge_rewind_choices(picker: &mut Picker, earlier: Vec<Choice>, pending_up: usize) -> bool {
    let visible = picker.visible_choices();
    let selected = visible
        .get(picker.selected)
        .and_then(|choice| rewind_event_id(choice));
    let mut choices: BTreeMap<_, _> = visible
        .into_iter()
        .filter_map(|choice| rewind_event_id(choice).map(|id| (id, choice.clone())))
        .collect();
    for choice in earlier {
        if let Some(id) = rewind_event_id(&choice) {
            choices.entry(id).or_insert(choice);
        }
    }
    let mut choices: Vec<_> = choices.into_values().collect();
    let limited = bound_rewind_choices(&mut choices, selected, RewindWindow::Earlier);
    let anchor = selected
        .and_then(|selected| {
            choices
                .iter()
                .position(|choice| rewind_event_id(choice) == Some(selected))
        })
        .unwrap_or_else(|| choices.len().saturating_sub(1));
    picker.set_older_choices(choices, anchor);
    if selected.is_some() && pending_up > 0 {
        picker.move_by(-(pending_up as isize));
    }
    limited
}
