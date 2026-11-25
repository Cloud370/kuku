use std::collections::{HashMap, VecDeque};

use kuku_client::{
    Acknowledgement, Action, AgentStatus, InputDelivery, InputSource, InputStatus, InputTiming,
    QueuedInput, Snapshot, Timing,
};

use crate::jobs::Selection;
use crate::menu::PromptKind;
use crate::text::cap;

use super::{Address, App, KeyboardFocus, Modal, PendingStage, SnapshotRefresh, SubmissionStatus};

const MAX_PREVIEW_BYTES: usize = 8 * 1024;
const MAX_SETTLED: usize = 256;
const MAX_TERMINAL_ROWS: usize = 3;

#[derive(Debug, Clone, Copy)]
pub(crate) enum QueueAction {
    Edit,
    Withdraw,
}

pub(super) struct PendingQueueAction {
    selection: Selection,
    input_id: String,
    draft_revision: u64,
    action: QueueAction,
}

#[derive(Debug)]
pub(crate) struct DeliveryRow {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) agent_sender: Option<String>,
    pub(crate) timing: Timing,
    pub(crate) status: SubmissionStatus,
    pub(crate) replacement: bool,
    pub(crate) withdrawal: Option<SubmissionStatus>,
    receipt: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct Deliveries {
    rows: Vec<DeliveryRow>,
    // Local sends made from an idle pane may receive a durable queued receipt in
    // the interval before their own execution admits them. Only the receipt and
    // a snapshot's execution identity can distinguish that from a true queue.
    starting: HashMap<String, Option<u64>>,
    settled: VecDeque<String>,
    snapshot_cursor: u64,
    observed_cursor: u64,
}

impl Deliveries {
    pub(crate) fn rows(&self) -> &[DeliveryRow] {
        &self.rows
    }

    pub(super) fn begin_idle_send(&mut self, id: &str) {
        self.starting.insert(id.to_owned(), None);
    }

    pub(super) fn starting(&self) -> bool {
        !self.starting.is_empty()
    }

    pub(super) fn awaiting_admission(&self) -> bool {
        self.starting.values().any(Option::is_some)
    }

    pub(super) fn hidden_queue_count(&self, snapshot: &Snapshot) -> usize {
        snapshot
            .pending
            .iter()
            .filter(|entry| {
                self.starting.contains_key(&entry.receipt.command_id)
                    || self.settled.contains(&entry.receipt.command_id)
            })
            .count()
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
                if ack.status == Some(InputStatus::Queued)
                    && let Some(starting) = self.starting.get_mut(id)
                    && (self.snapshot_cursor < ack.receipt.event_id
                        || *starting == Some(ack.receipt.event_id))
                {
                    *starting = Some(ack.receipt.event_id);
                    return;
                }
                self.starting.remove(id);
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
        for entry in &snapshot.pending {
            let id = &entry.receipt.command_id;
            if let Some(starting) = self.starting.get_mut(id) {
                if snapshot.agent.status == AgentStatus::Running
                    && !snapshot.cleaning_up
                    && snapshot.execution_id().is_some_and(|execution| {
                        execution > entry.receipt.event_id
                            && snapshot.agent.turn.is_none_or(|turn| turn < execution)
                    })
                {
                    *starting = Some(entry.receipt.event_id);
                } else {
                    // Another execution was already running when this send
                    // arrived, or scheduling is blocked. Show the real queue.
                    self.starting.remove(id);
                }
            }
        }
        self.starting.retain(|id, receipt| {
            receipt.is_none_or(|receipt| {
                receipt > snapshot.cursor
                    || snapshot
                        .pending
                        .iter()
                        .any(|entry| entry.receipt.command_id == *id)
            })
        });
        let mut queue: HashMap<_, _> = snapshot
            .pending
            .iter()
            .filter(|entry| {
                listed_input(entry) && !self.starting.contains_key(&entry.receipt.command_id)
            })
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
                let row = self.push(
                    id,
                    &entry.input.text,
                    input_timing(entry),
                    SubmissionStatus::Queued,
                    Some(entry.receipt.event_id),
                    false,
                );
                if let InputSource::Agent(sender) = &entry.input.source {
                    row.agent_sender = Some(sender.clone());
                }
            }
        }
        // A receipt can precede the snapshot that contains earlier inputs from
        // other windows. Reestablish the queue's server-owned order when known.
        let order: HashMap<_, _> = snapshot
            .pending
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.receipt.command_id.as_str(), index))
            .collect();
        self.rows
            .sort_by_key(|row| order.get(row.id.as_str()).copied().unwrap_or(usize::MAX));
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
        self.starting.remove(id);
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
    ) -> &mut DeliveryRow {
        let prefix = cap(text, MAX_PREVIEW_BYTES);
        let text = if text.len() == prefix.len() {
            prefix.to_owned()
        } else {
            format!("{prefix}\n…")
        };
        self.rows.push(DeliveryRow {
            id: id.to_owned(),
            text,
            agent_sender: None,
            timing,
            status,
            replacement,
            receipt,
            withdrawal: None,
        });
        self.rows.last_mut().expect("delivery row just added")
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

fn listed_input(entry: &QueuedInput) -> bool {
    ordinary_input(entry) || matches!(entry.input.source, InputSource::Agent(_))
}

impl App {
    pub(crate) fn queue_input_ids(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.pane()
            .deliveries
            .rows()
            .iter()
            .filter(|row| {
                row.status == SubmissionStatus::Queued
                    && (self.snapshot_queue_input(&row.id).is_some()
                        || self.pending_queue_receipt(&row.id).is_some())
            })
            .map(|row| row.id.as_str())
    }

    pub(crate) fn queue_input_available(&self, id: &str) -> bool {
        self.listed_queued_input(id).is_some() || self.pending_queue_receipt(id).is_some()
    }

    pub(crate) fn queue_input_editable(&self, id: &str) -> bool {
        self.listed_queued_input(id).is_some_and(ordinary_input)
            || self.pending_queue_receipt(id).is_some()
    }

    pub(crate) fn cancel_queue_action(&mut self) {
        self.pending_queue_action = None;
    }

    pub(crate) fn request_queue_action(&mut self, id: String, action: QueueAction) {
        self.pending_queue_action = None;
        if self.queue_mutation_pending(&id) {
            self.problem("A change to this queued input is already pending");
            return;
        }
        if let Some(entry) = self.listed_queued_input(&id) {
            match action {
                QueueAction::Edit if ordinary_input(entry) => self.edit_queued(id),
                QueueAction::Edit => {
                    self.problem("Agent messages cannot be edited · Delete to withdraw")
                }
                QueueAction::Withdraw => self.withdraw_queued(id),
            }
            return;
        }
        if let Some(receipt) = self.pending_queue_receipt(&id) {
            self.pending_queue_action = Some(PendingQueueAction {
                selection: self.selection(),
                input_id: id,
                draft_revision: self.pane().draft_revision,
                action,
            });
            self.confirmation("Reading queued input…");
            self.request_refresh(SnapshotRefresh::AfterCommit(receipt));
        } else {
            self.focus_composer();
            self.pane_mut().selected_queue_input = None;
            self.problem("Selected input is no longer queued");
        }
    }

    pub(super) fn resume_queue_action(&mut self) {
        let Some(pending) = self.pending_queue_action.take() else {
            return;
        };
        if pending.selection != self.selection()
            || pending.draft_revision != self.pane().draft_revision
            || self.pane().keyboard_focus != KeyboardFocus::Queue
            || self.pane().selected_queue_input.as_deref() != Some(pending.input_id.as_str())
            || self.modal.is_some()
            || !self.available()
            || self.quit
        {
            return;
        }
        if self.pending_queue_receipt(&pending.input_id).is_some()
            && self.editable_queued_input(&pending.input_id).is_none()
        {
            self.pending_queue_action = Some(pending);
            return;
        }
        self.request_queue_action(pending.input_id, pending.action);
    }

    pub(crate) fn has_queued_input(&self) -> bool {
        self.queue_input_ids().next().is_some()
    }

    fn editable_queued_input(&self, id: &str) -> Option<&QueuedInput> {
        self.listed_queued_input(id)
            .filter(|entry| ordinary_input(entry))
    }

    pub(crate) fn listed_queued_input(&self, id: &str) -> Option<&QueuedInput> {
        let entry = self.snapshot_queue_input(id)?;
        self.pane()
            .deliveries
            .rows()
            .iter()
            .any(|row| row.id == id && row.status == SubmissionStatus::Queued)
            .then_some(entry)
    }

    fn snapshot_queue_input(&self, id: &str) -> Option<&QueuedInput> {
        self.pane()
            .snapshot
            .as_ref()?
            .pending
            .iter()
            .find(|entry| entry.receipt.command_id == id && listed_input(entry))
    }

    pub(crate) fn edit_queued(&mut self, input_command_id: String) {
        if self.queue_mutation_pending(&input_command_id) {
            self.problem("A change to this queued input is already pending");
            return;
        }
        let text = self
            .editable_queued_input(&input_command_id)
            .map(|entry| entry.input.clone());
        let Some(input) = text else {
            self.problem("Input has already left the queue");
            return;
        };
        if input.images.is_empty() {
            self.open_queued_editor(
                input_command_id,
                kuku_client::MessageDraft {
                    text: input.text,
                    images: Vec::new(),
                },
            );
        } else if self.require_service() {
            self.read_queued_draft(input_command_id, QueueAction::Edit);
        }
    }

    pub(crate) fn open_queued_editor(
        &mut self,
        input_command_id: String,
        draft: kuku_client::MessageDraft,
    ) {
        self.prompt(
            PromptKind::QueuedInput {
                input_command_id,
                images: draft.images,
            },
            "Edit queued input",
            "Enter replaces the queued text · Shift+Enter adds a line · Esc keeps the queued input",
        );
        if let Some(Modal::Prompt(prompt)) = &mut self.modal
            && let PromptKind::QueuedInput { images, .. } = &prompt.kind
        {
            prompt
                .editor
                .set_image_markers(&draft.text, images.iter().map(|image| &image.marker));
        }
    }

    pub(crate) fn replace_queued(
        &mut self,
        input_command_id: String,
        text: String,
        draft_images: Vec<kuku_client::ImageAttachment>,
    ) -> bool {
        if self.editable_queued_input(&input_command_id).is_none() {
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
                Action::ReplaceQueued { input_command_id: pending, .. } | Action::Withdraw { input_command_id: pending, .. }
                    if pending == &input_command_id)
                && !matches!(item.stage, PendingStage::Rejected(_))
        }) {
            self.problem("Check the pending change with delivery recovery before submitting another edit");
            return false;
        }
        self.issue(
            address,
            Action::ReplaceQueued {
                input_command_id,
                text,
                image_markers: draft_images
                    .iter()
                    .map(|image| image.marker.clone())
                    .collect(),
                draft_images,
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
        if let Action::Withdraw {
            input_command_id, ..
        } = &item.command.action
        {
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
        if matches!(item.command.action, Action::Send { .. }) && item.stage.needs_recovery() {
            pane.deliveries.starting.remove(id);
        }
        // Keep healthy ordinary submissions in private pending storage, not the visible
        // queue. A durable queue receipt or a real recovery transition creates a row.
        // Retain a row already exposed during recovery or confirmed by a snapshot.
        if matches!(item.command.action, Action::Send { .. })
            && matches!(item.stage, PendingStage::Saving | PendingStage::Sending)
            && !pane.deliveries.rows().iter().any(|row| row.id == id)
        {
            return;
        }
        let (text, timing, replacement) = match &item.command.action {
            Action::Send { text, timing, .. } => (text, *timing, false),
            Action::ReplaceQueued {
                input_command_id,
                text,
                ..
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

    fn pending_queue_receipt(&self, id: &str) -> Option<u64> {
        let deliveries = &self.pane().deliveries;
        deliveries
            .rows()
            .iter()
            .find(|row| row.id == id && row.status == SubmissionStatus::Queued)?
            .receipt
            .filter(|receipt| *receipt > deliveries.snapshot_cursor)
    }
}
