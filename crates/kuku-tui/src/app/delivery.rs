use std::collections::{HashMap, VecDeque};

use kuku_client::{
    Acknowledgement, Action, InputDelivery, InputSource, InputStatus, InputTiming, QueuedInput,
    Snapshot, Timing,
};

use crate::menu::PromptKind;
use crate::text::cap;

use super::{Address, App, Modal, PendingStage, SubmissionStatus};

const MAX_PREVIEW_BYTES: usize = 8 * 1024;
const MAX_SETTLED: usize = 256;
const MAX_TERMINAL_ROWS: usize = 3;

#[derive(Debug)]
pub(crate) struct DeliveryRow {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) timing: Timing,
    pub(crate) status: SubmissionStatus,
    pub(crate) replacement: bool,
    pub(crate) withdrawal: Option<SubmissionStatus>,
    receipt: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct Deliveries {
    rows: Vec<DeliveryRow>,
    settled: VecDeque<String>,
    snapshot_cursor: u64,
    observed_cursor: u64,
}

impl Deliveries {
    pub(crate) fn rows(&self) -> &[DeliveryRow] {
        &self.rows
    }

    pub(super) fn local(
        &mut self,
        id: &str,
        text: &str,
        timing: Timing,
        status: SubmissionStatus,
        replacement: bool,
    ) {
        if self.settled.iter().any(|known| known == id) {
            return;
        }
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            if row.receipt.is_none() {
                row.status = status;
            }
        } else {
            self.push(id, text, timing, status, None, replacement);
        }
        self.trim_terminal_rows();
    }

    pub(super) fn receipt(
        &mut self,
        ack: &Acknowledgement,
        text: &str,
        timing: Timing,
        replacement: bool,
    ) {
        let id = &ack.receipt.command_id;
        match ack.status {
            Some(InputStatus::Accepted | InputStatus::Withdrawn) => self.settle(id),
            Some(InputStatus::Queued) | None => {
                if self.settled.iter().any(|known| known == id) {
                    return;
                }
                if ack.receipt.event_id <= self.snapshot_cursor
                    && !self
                        .rows
                        .iter()
                        .any(|row| row.id == *id && row.receipt.is_some())
                {
                    self.remove(id);
                    return;
                }
                if let Some(row) = self.rows.iter_mut().find(|row| row.id == *id) {
                    row.status = SubmissionStatus::from_receipt(ack.status);
                    row.receipt = Some(ack.receipt.event_id);
                } else if ack.receipt.event_id > self.snapshot_cursor {
                    self.push(
                        id,
                        text,
                        timing,
                        SubmissionStatus::from_receipt(ack.status),
                        Some(ack.receipt.event_id),
                        replacement,
                    );
                }
            }
        }
    }

    pub(super) fn snapshot(&mut self, snapshot: &Snapshot) {
        if snapshot.cursor < self.observed_cursor || snapshot.cursor < self.snapshot_cursor {
            return;
        }
        self.snapshot_cursor = snapshot.cursor;
        let mut queue: HashMap<_, _> = snapshot
            .pending
            .iter()
            .filter(|entry| ordinary_input(entry))
            .map(|entry| (entry.receipt.command_id.as_str(), entry))
            .collect();
        self.rows.retain(|row| {
            !matches!(
                row.status,
                SubmissionStatus::Queued | SubmissionStatus::Confirmed
            ) || row.receipt.is_none_or(|receipt| receipt > snapshot.cursor)
                || queue.contains_key(row.id.as_str())
        });
        for row in &mut self.rows {
            if let Some(entry) = queue.remove(row.id.as_str()) {
                row.status = SubmissionStatus::Queued;
                row.receipt = Some(entry.receipt.event_id);
                row.timing = input_timing(entry);
            }
        }
        for entry in &snapshot.pending {
            let id = &entry.receipt.command_id;
            if queue.contains_key(id.as_str()) && !self.settled.iter().any(|known| known == id) {
                self.push(
                    id,
                    &entry.input.text,
                    input_timing(entry),
                    SubmissionStatus::Queued,
                    Some(entry.receipt.event_id),
                    false,
                );
            }
        }
    }

    pub(super) fn committed(&mut self, cursor: u64, delivery: InputDelivery) {
        self.observed_cursor = self.observed_cursor.max(cursor);
        match delivery {
            InputDelivery::Accepted { input_ids } => {
                for id in input_ids {
                    self.settle(&id);
                }
            }
            InputDelivery::Withdrawn { input_id } => self.settle(&input_id),
            InputDelivery::Replaced {
                input_id,
                replacement_id,
            } => self.replace(&input_id, &replacement_id, cursor),
        }
    }

    pub(super) fn withdrawal(&mut self, id: &str, status: Option<SubmissionStatus>) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            row.withdrawal = status;
        }
    }

    pub(super) fn acknowledge_withdrawal(&mut self, id: &str, status: Option<InputStatus>) {
        match status {
            Some(InputStatus::Accepted | InputStatus::Withdrawn) => self.settle(id),
            Some(InputStatus::Queued) | None => self.withdrawal(id, None),
        }
    }

    pub(super) fn replace(&mut self, original: &str, replacement: &str, receipt: u64) {
        let original_position = self.rows.iter().position(|row| row.id == original);
        self.settle(original);
        if self.settled.iter().any(|id| id == replacement) {
            return;
        }
        if let Some(index) = self.rows.iter().position(|row| row.id == replacement) {
            let mut row = self.rows.remove(index);
            row.status = SubmissionStatus::Queued;
            row.receipt = Some(receipt);
            self.rows
                .insert(original_position.unwrap_or(index).min(self.rows.len()), row);
        }
    }

    pub(super) fn remove(&mut self, id: &str) {
        self.rows.retain(|row| row.id != id);
    }

    fn settle(&mut self, id: &str) {
        self.remove(id);
        self.remember_settled(id);
    }

    fn push(
        &mut self,
        id: &str,
        text: &str,
        timing: Timing,
        status: SubmissionStatus,
        receipt: Option<u64>,
        replacement: bool,
    ) {
        let prefix = cap(text, MAX_PREVIEW_BYTES);
        let text = if text.len() == prefix.len() {
            prefix.to_owned()
        } else {
            format!("{prefix}\n…")
        };
        self.rows.push(DeliveryRow {
            id: id.to_owned(),
            text,
            timing,
            status,
            replacement,
            receipt,
            withdrawal: None,
        });
    }

    fn remember_settled(&mut self, id: &str) {
        if !self.settled.iter().any(|known| known == id) {
            self.settled.push_front(id.to_owned());
            self.settled.truncate(MAX_SETTLED);
        }
    }

    fn trim_terminal_rows(&mut self) {
        let terminal = |row: &DeliveryRow| row.status == SubmissionStatus::Rejected;
        let mut excess = self
            .rows
            .iter()
            .filter(|row| terminal(row))
            .count()
            .saturating_sub(MAX_TERMINAL_ROWS);
        self.rows.retain(|row| {
            if excess > 0 && terminal(row) {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
}

pub(super) fn input_timing(entry: &QueuedInput) -> Timing {
    match entry.input.timing {
        InputTiming::NextStep => Timing::Steer,
        InputTiming::NextTurn => Timing::NextTurn,
    }
}

pub(super) fn ordinary_input(entry: &QueuedInput) -> bool {
    matches!(&entry.input.source, InputSource::Host(caller) if caller == "http")
}

impl App {
    pub(crate) fn has_queued_input(&self) -> bool {
        self.pane()
            .deliveries
            .rows()
            .iter()
            .any(|row| row.status == SubmissionStatus::Queued)
    }

    pub(crate) fn queued_for_edit(&self) -> impl Iterator<Item = &QueuedInput> {
        let pane = self.pane();
        pane.snapshot
            .iter()
            .flat_map(|snapshot| &snapshot.pending)
            .filter(|entry| {
                ordinary_input(entry)
                    && pane.deliveries.rows().iter().any(|row| {
                        row.id == entry.receipt.command_id && row.status == SubmissionStatus::Queued
                    })
            })
    }

    pub(crate) fn edit_queued(&mut self, input_command_id: String) {
        if self.queue_mutation_pending(&input_command_id) {
            self.problem("A change to this queued input is already pending");
            return;
        }
        let text = self
            .queued_for_edit()
            .find(|entry| entry.receipt.command_id == input_command_id)
            .map(|entry| entry.input.text.clone());
        let Some(text) = text else {
            self.problem("Input has already left the queue");
            return;
        };
        self.prompt(
            PromptKind::QueuedInput { input_command_id },
            "Edit queued input",
            "Enter replaces the queued text · Shift+Enter adds a line · Esc keeps the queued input",
        );
        if let Some(Modal::Prompt(prompt)) = &mut self.modal {
            prompt.editor.set_text(text);
        }
    }

    pub(crate) fn replace_queued(&mut self, input_command_id: String, text: String) -> bool {
        if !self
            .queued_for_edit()
            .any(|entry| entry.receipt.command_id == input_command_id)
        {
            self.problem("Selected input is no longer queued");
            return false;
        }
        if self.queue_mutation_pending(&input_command_id) {
            self.problem("A change to this queued input is already pending");
            return false;
        }
        if !self.require_service() {
            return false;
        }
        let address = self.active.clone();
        let target = self.target(&address);
        if self.pending.values().any(|item| {
            item.command.target == target && matches!(&item.command.action,
                Action::ReplaceQueued { input_command_id: pending, .. } | Action::Withdraw { input_command_id: pending }
                    if pending == &input_command_id)
                && !matches!(item.stage, PendingStage::Rejected(_))
        }) {
            self.problem("Check the pending change in /pending before submitting another edit");
            return false;
        }
        self.issue(
            address,
            Action::ReplaceQueued {
                input_command_id,
                text,
            },
        );
        let pane = self.pane_mut();
        pane.keyboard_focus = super::KeyboardFocus::Composer;
        pane.selected_queue_input = None;
        true
    }

    pub(super) fn update_pending_delivery(&mut self, id: &str) {
        let Some(item) = self.pending.get(id) else {
            return;
        };
        if let Action::Withdraw { input_command_id } = &item.command.action {
            let address = Address::from_target(&item.command.target);
            if let Some(pane) = self.panes.get_mut(&address) {
                pane.deliveries.withdrawal(
                    input_command_id,
                    Some(SubmissionStatus::from_pending(&item.stage)),
                );
            }
            return;
        }
        if !matches!(
            item.command.action,
            Action::Send { .. } | Action::ReplaceQueued { .. }
        ) {
            return;
        }
        let address = Address::from_target(&item.command.target);
        let pane = self.panes.entry(address).or_default();
        let (text, timing, replacement) = match &item.command.action {
            Action::Send { text, timing, .. } => (text, *timing, false),
            Action::ReplaceQueued {
                input_command_id,
                text,
            } => {
                let timing = pane
                    .deliveries
                    .rows()
                    .iter()
                    .find(|row| row.id == *input_command_id)
                    .map_or(Timing::Steer, |row| row.timing);
                (text, timing, true)
            }
            _ => return,
        };
        if item.origin.is_none()
            && matches!(item.stage, PendingStage::Checking)
            && !pane.deliveries.rows().iter().any(|row| row.id == id)
        {
            return;
        }
        pane.deliveries.local(
            id,
            text,
            timing,
            SubmissionStatus::from_pending(&item.stage),
            replacement,
        );
    }
}
