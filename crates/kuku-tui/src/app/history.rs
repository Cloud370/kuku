use std::collections::BTreeMap;

use kuku_client::{Error, HistoryPage, RewindGroupPage};

use crate::jobs::{Job, Selection};
use crate::menu::{ChoiceKind, Ticket};
use crate::transcript::{HistoryRequest, RewindGroupRequest};

use super::rewind::{rewind_choice, rewind_history_header};
use super::App;

impl App {
    pub(super) fn apply_rewind_history_update(
        &mut self,
        selection: Selection,
        ticket: Ticket,
        before: u64,
        result: Result<HistoryPage, Error>,
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
        if history.selection != selection || history.pending_before != Some(before) {
            return;
        }
        history.pending_before = None;
        picker.loading = false;
        match result {
            Ok(page) if page.next_before.is_none_or(|next| next < before) => {
                history.next_before = page.next_before;
                picker.header = rewind_history_header(page.next_before, false).into();
                picker.problem = None;
                merge_rewind_page(picker, page, before, &selection.target.agent);
            }
            Ok(_) => {
                picker.problem = Some("History did not advance · ↑ at the top retries".into());
                picker.header = rewind_history_header(history.next_before, false).into();
            }
            Err(error) => {
                picker.problem = Some(format!("History · {error} · ↑ at the top retries"));
                picker.header = rewind_history_header(history.next_before, false).into();
            }
        }
    }

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

fn merge_rewind_page(
    picker: &mut crate::menu::Picker,
    page: HistoryPage,
    before: u64,
    agent: &str,
) {
    let selected = match picker.selected_choice() {
        Some(ChoiceKind::RewindMessage { event_id, .. }) => Some(event_id),
        _ => None,
    };
    let mut choices: BTreeMap<_, _> = picker
        .visible_choices()
        .into_iter()
        .filter_map(|choice| match choice.kind {
            ChoiceKind::RewindMessage { event_id, .. } => Some((event_id, choice.clone())),
            _ => None,
        })
        .collect();
    for entry in page.entries.into_iter().filter(|entry| {
        entry.id < before && entry.agent == agent && entry.kind == kuku_client::HistoryKind::User
    }) {
        let Some(turn) = entry
            .details
            .get("turn_number")
            .and_then(serde_json::Value::as_u64)
            .or(entry.turn)
        else {
            continue;
        };
        choices
            .entry(entry.id)
            .or_insert_with(|| rewind_choice(entry.id, turn, entry.text.unwrap_or_default()));
    }
    let choices: Vec<_> = choices.into_values().collect();
    let selected = selected
        .and_then(|selected| choices.iter().position(|choice| matches!(choice.kind, ChoiceKind::RewindMessage { event_id, .. } if event_id == selected)))
        .unwrap_or_else(|| choices.len().saturating_sub(1));
    picker.set_local(choices);
    picker.selected = selected;
}
