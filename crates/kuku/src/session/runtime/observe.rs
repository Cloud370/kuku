use std::collections::{HashMap, VecDeque};
use std::path::Path;

use tokio::sync::broadcast;

use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::query::UiEvent;
use crate::Result;

use super::record::{validate_key, Record};
use super::{load_state, AgentSnapshot, AgentView, InputStatus, PendingInput, Receipt, Session};

#[derive(Debug, Clone)]
pub(super) struct LiveObservation {
    agent: String,
    execution_id: u64,
    turn: u64,
    after: u64,
    event: UiEvent,
}

impl From<LiveObservation> for Observation {
    fn from(live: LiveObservation) -> Self {
        Self::Live {
            agent: live.agent,
            execution_id: live.execution_id,
            turn: live.turn,
            after: live.after,
            event: Box::new(live.event),
        }
    }
}

/// A committed fact or a bounded transient update from the same runtime.
#[derive(Debug, Clone)]
pub enum Observation {
    /// A durable event whose identity is a reconnectable cursor.
    Committed(Box<StoredEvent>),
    /// A transient streaming update that does not advance the durable cursor.
    Live {
        /// Stable address that emitted this update.
        agent: String,
        /// Durable identity of the execution that emitted this update.
        execution_id: u64,
        /// Turn that emitted this update.
        turn: u64,
        /// Latest committed cursor when this update was published.
        after: u64,
        /// Non-durable runtime update.
        event: Box<UiEvent>,
    },
    /// Slow consumption dropped transient updates while durable facts remain available.
    Lagged { skipped: u64 },
}

/// Cursor-based observer that never owns advancement or an unbounded event queue.
pub struct SessionObserver {
    session: Session,
    cursor: u64,
    page: VecDeque<StoredEvent>,
    live: broadcast::Receiver<LiveObservation>,
}

/// Per-Agent display numbers derived from the committed acceptance of new Turns.
#[derive(Debug, Default)]
pub struct TurnIndex {
    by_agent: HashMap<String, Vec<u64>>,
}

impl TurnIndex {
    /// Read Turn numbers and their source cursor without opening a Session.
    pub fn read(events_path: &Path) -> Result<(Self, u64)> {
        let mut index = Self::default();
        let cursor = EventStore::visit_path(events_path, |event| {
            if !matches!(&event.payload, EventPayload::Unknown(payload)
                if payload["kind"] == "runtime.operation"
                    && payload["record"]["operation"] == "inputs_accepted")
            {
                return Ok(());
            }
            if let Some(Record::InputsAccepted { address, turn, .. }) = Record::from_event(&event)?
            {
                index.accepted(address, event.id, turn);
            }
            Ok(())
        })?;
        Ok((index, cursor))
    }

    /// Return the one-based display number for a known Turn belonging to an Agent.
    pub fn number(&self, address: &str, turn: u64) -> Option<u64> {
        let position = self.by_agent.get(address)?.binary_search(&turn).ok()?;
        Some(position as u64 + 1)
    }

    pub(super) fn accepted(&mut self, address: String, event_id: u64, existing_turn: Option<u64>) {
        if existing_turn.is_none() {
            self.by_agent.entry(address).or_default().push(event_id);
        }
    }
}

impl Session {
    /// Return the one-based display number for a committed Agent Turn identity.
    pub fn turn_number(&self, address: &str, turn: u64) -> Option<u64> {
        self.inner.state.lock().unwrap().turns.number(address, turn)
    }

    /// Return a selected agent's state, queued deliveries, and the matching journal cursor.
    pub fn agent_view(&self, address: &str) -> Result<AgentView> {
        let state = self.inner.state.lock().unwrap();
        let cursor = if state.closed {
            state.closed_cursor
        } else {
            self.current_cursor()?
        };
        state.view(address, cursor)
    }

    /// Return a selected agent and the latest durable cursor in one observation.
    pub fn agent_with_cursor(&self, address: &str) -> Result<(AgentSnapshot, u64)> {
        let state = self.inner.state.lock().unwrap();
        let cursor = if state.closed {
            state.closed_cursor
        } else {
            self.current_cursor()?
        };
        Ok((state.agent(address)?.snapshot.clone(), cursor))
    }

    /// Read a closed session's selected snapshot and pending inputs without opening runtime resources.
    pub fn read_view(events_path: &Path, address: &str) -> Result<AgentView> {
        let (state, cursor) = load_state(events_path)?;
        state.view(address, cursor)
    }

    /// Read an original command receipt and current input state from a closed session.
    pub fn read_receipt(
        events_path: &Path,
        command_id: &str,
    ) -> Result<Option<(Receipt, Option<InputStatus>)>> {
        validate_key(command_id)?;
        let (state, _) = load_state(events_path)?;
        Ok(state
            .commands
            .get(command_id)
            .map(|(_, receipt)| (receipt.clone(), state.deliveries.get(command_id).copied())))
    }

    /// List retractable deliveries awaiting acceptance in receipt order.
    pub fn pending_inputs(&self, address: &str) -> Result<Vec<PendingInput>> {
        self.inner.state.lock().unwrap().pending_inputs(address)
    }

    /// Return the current durable state of a particular input delivery.
    pub fn input_status(&self, address: &str, command_id: &str) -> Result<InputStatus> {
        self.inner
            .state
            .lock()
            .unwrap()
            .input_status(address, command_id)
    }

    /// List executions with an owned worker, including those still in cleanup.
    pub fn active_executions(&self) -> Vec<(String, u64)> {
        let state = self.inner.state.lock().unwrap();
        state
            .agents
            .values()
            .filter_map(|agent| {
                agent
                    .active_epoch
                    .and(agent.snapshot.execution_id)
                    .map(|execution| (agent.snapshot.address.clone(), execution))
            })
            .collect()
    }

    pub(super) fn publish(&self, address: &str, execution_id: u64, turn: u64, event: UiEvent) {
        if let Ok(after) = self.current_cursor() {
            let _ = self.inner.live.send(LiveObservation {
                agent: address.into(),
                execution_id,
                turn,
                after,
                event,
            });
        }
    }

    fn current_cursor(&self) -> Result<u64> {
        let writer = self.inner.writer.lock().unwrap();
        match writer.as_ref() {
            Some(writer) => Ok(writer.last_id()),
            None => EventStore::read_head(self.events_path()),
        }
    }
}

impl SessionObserver {
    /// Return the last durable event delivered by this observer.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Return one already available observation without waiting or advancing execution.
    pub fn try_next(&mut self) -> Result<Option<Observation>> {
        if self.page.is_empty() {
            self.page = self.session.events(self.cursor, 32)?.into();
        }
        if let Some(event) = self.page.pop_front() {
            self.cursor = event.id;
            return Ok(Some(Observation::Committed(Box::new(event))));
        }
        match self.live.try_recv() {
            Ok(live) => Ok(Some(live.into())),
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                Ok(Some(Observation::Lagged { skipped }))
            }
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                Ok(None)
            }
        }
    }

    /// Wait for committed facts or transient progress without driving execution.
    pub async fn next(&mut self) -> Result<Option<Observation>> {
        loop {
            if let Some(event) = self.page.pop_front() {
                self.cursor = event.id;
                return Ok(Some(Observation::Committed(Box::new(event))));
            }
            let changed = self
                .session
                .inner
                .writer
                .lock()
                .unwrap()
                .as_ref()
                .map(|writer| writer.changed());
            let changed =
                changed.unwrap_or_else(|| std::sync::Arc::new(tokio::sync::Notify::new()));
            let notified = changed.notified_owned();
            tokio::pin!(notified);
            notified.as_mut().enable();
            self.page = self.session.events(self.cursor, 32)?.into();
            if !self.page.is_empty() {
                continue;
            }
            if self.session.inner.state.lock().unwrap().closed {
                return Ok(None);
            }
            tokio::select! {
                _ = &mut notified => {}
                result = self.live.recv() => match result {
                    Ok(live) => return Ok(Some(live.into())),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => return Ok(Some(Observation::Lagged { skipped })),
                    Err(broadcast::error::RecvError::Closed) => return Ok(None),
                }
            }
        }
    }

    pub(super) fn new(session: Session, cursor: u64) -> Self {
        let live = session.inner.live.subscribe();
        Self {
            session,
            cursor,
            page: VecDeque::new(),
            live,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{EventPayload, EventStore, Record, TurnIndex};

    #[test]
    fn admitted_turns_are_numbered_per_agent_from_persisted_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        for id in 1..=23 {
            let input = match id {
                6 | 23 => Some(("main", None)),
                10 => Some(("main", Some(6))),
                14 => Some(("reviewer", None)),
                _ => None,
            };
            let event = if let Some((address, turn)) = input {
                Record::InputsAccepted {
                    address: address.to_owned(),
                    input_ids: vec![format!("send-{id}")],
                    turn,
                }
                .payload()
                .unwrap()
            } else {
                EventPayload::Unknown(json!({"kind":"fixture","ts":"2026-01-01T00:00:00Z"}))
            };
            assert_eq!(id, store.append(event).unwrap().id);
        }
        drop(store);

        let (index, cursor) = TurnIndex::read(&path).unwrap();
        assert_eq!(23, cursor);
        assert_eq!(Some(1), index.number("main", 6));
        assert_eq!(Some(2), index.number("main", 23));
        assert_eq!(None, index.number("main", 10));
        assert_eq!(None, index.number("main", 14));
        assert_eq!(Some(1), index.number("reviewer", 14));
    }
}
