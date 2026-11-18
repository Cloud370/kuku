use std::collections::{HashMap, VecDeque};
use std::path::Path;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::query::UiEvent;
use crate::Result;

use super::record::{validate_key, Record, State};
use super::{
    load_state, AgentSnapshot, AgentStatus, AgentView, InputStatus, PendingInput, Receipt, Session,
};

const REPLY_PREVIEW_BYTES: usize = 64 * 1024;
const PEER_PREVIEW_BYTES: usize = 512;

/// An in-memory prefix of the active model reply, containing only observed text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplyPreview {
    /// Execution owning the current model request.
    pub execution_id: u64,
    /// Turn of the active model request.
    pub turn: u64,
    /// UTF-8 prefix from the start of this request's reply.
    pub text: String,
    /// The reply already exceeded the preview's byte limit.
    pub truncated: bool,
}

impl ReplyPreview {
    fn new(execution_id: u64, turn: u64) -> Self {
        Self {
            execution_id,
            turn,
            text: String::new(),
            truncated: false,
        }
    }

    fn append(&mut self, text: &str) {
        if self.truncated {
            return;
        }
        let end = text.floor_char_boundary(text.len().min(REPLY_PREVIEW_BYTES - self.text.len()));
        self.text.push_str(&text[..end]);
        self.truncated = end < text.len();
    }
}

/// Active reply previews captured with one Session cursor.
#[derive(Debug, Default)]
pub struct ReplyPreviews {
    /// The selected Agent's active reply.
    pub selected: Option<ReplyPreview>,
    /// Bounded excerpts for the other streaming Agents, indexed by address.
    pub peers: HashMap<String, String>,
}

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
    /// Current active reply at an Agent-specific attachment boundary.
    ReplyPreview {
        /// Stable address producing this reply.
        agent: String,
        /// Latest committed cursor when the prefix and subscription were captured together.
        after: u64,
        /// The replacement prefix. Subsequent live events carry new deltas only.
        reply: Box<ReplyPreview>,
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
    preview_agent: Option<String>,
    preview: Option<Observation>,
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

    /// Return an Agent's durable view and active replies at the same observer boundary.
    pub fn agent_view_with_replies(&self, address: &str) -> Result<(AgentView, ReplyPreviews)> {
        let state = self.inner.state.lock().unwrap();
        let cursor = if state.closed {
            state.closed_cursor
        } else {
            self.current_cursor()?
        };
        let view = state.view(address, cursor)?;
        let previews = ReplyPreviews {
            selected: reply_preview(&state, address).cloned(),
            peers: state
                .agents
                .iter()
                .filter(|(peer, _)| peer.as_str() != address)
                .filter_map(|(peer, _)| {
                    let text = &reply_preview(&state, peer)?.text;
                    let end = text.floor_char_boundary(text.len().min(PEER_PREVIEW_BYTES));
                    Some((peer.clone(), text[..end].to_owned()))
                })
                .collect(),
        };
        Ok((view, previews))
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
        let mut state = self.inner.state.lock().unwrap();
        self.publish_locked(&mut state, address, execution_id, turn, event);
    }

    pub(crate) fn discard_reply_preview(&self, address: &str, execution_id: u64) {
        let mut state = self.inner.state.lock().unwrap();
        if let Some(agent) = state.agents.get_mut(address) {
            if agent.snapshot.execution_id == Some(execution_id) {
                agent.reply_preview = None;
            }
        }
    }

    pub(super) fn publish_locked(
        &self,
        state: &mut State,
        address: &str,
        execution_id: u64,
        turn: u64,
        event: UiEvent,
    ) {
        if let Ok(after) = self.current_cursor() {
            if let Some(agent) = state.agents.get_mut(address).filter(|agent| {
                agent.snapshot.status == AgentStatus::Running
                    && agent.snapshot.execution_id == Some(execution_id)
                    && agent.snapshot.turn == Some(turn)
            }) {
                match &event {
                    UiEvent::TextDelta { text } if !text.is_empty() => {
                        agent
                            .reply_preview
                            .get_or_insert_with(|| ReplyPreview::new(execution_id, turn))
                            .append(text);
                    }
                    UiEvent::ThinkingDelta { text } if !text.is_empty() => {
                        agent
                            .reply_preview
                            .get_or_insert_with(|| ReplyPreview::new(execution_id, turn));
                    }
                    UiEvent::ModelRequest { .. } | UiEvent::ModelRecovery { .. } => {
                        agent.reply_preview = None;
                    }
                    _ => {}
                }
            }
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

fn reply_preview<'a>(state: &'a State, address: &str) -> Option<&'a ReplyPreview> {
    let agent = state.agents.get(address)?;
    agent.reply_preview.as_ref().filter(|reply| {
        agent.snapshot.status == AgentStatus::Running
            && agent.snapshot.execution_id == Some(reply.execution_id)
            && agent.snapshot.turn == Some(reply.turn)
    })
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
        if let Some(preview) = self.preview.take() {
            return Ok(Some(preview));
        }
        match self.live.try_recv() {
            Ok(live) => Ok(Some(live.into())),
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                self.reattach();
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
            if let Some(preview) = self.preview.take() {
                return Ok(Some(preview));
            }
            if self.session.inner.state.lock().unwrap().closed {
                return Ok(None);
            }
            tokio::select! {
                _ = &mut notified => {}
                result = self.live.recv() => match result {
                    Ok(live) => return Ok(Some(live.into())),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        self.reattach();
                        return Ok(Some(Observation::Lagged { skipped }));
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(None),
                }
            }
        }
    }

    pub(super) fn new(session: Session, cursor: u64, preview_agent: Option<String>) -> Self {
        let (live, preview) = Self::attach(&session, preview_agent.as_deref());
        Self {
            session,
            cursor,
            page: VecDeque::new(),
            live,
            preview_agent,
            preview,
        }
    }

    fn reattach(&mut self) {
        if self.preview_agent.is_some() {
            (self.live, self.preview) = Self::attach(&self.session, self.preview_agent.as_deref());
        }
    }

    fn attach(
        session: &Session,
        address: Option<&str>,
    ) -> (broadcast::Receiver<LiveObservation>, Option<Observation>) {
        let Some(address) = address else {
            return (session.inner.live.subscribe(), None);
        };
        let state = session.inner.state.lock().unwrap();
        let live = session.inner.live.subscribe();
        let preview = reply_preview(&state, address).and_then(|reply| {
            Some(Observation::ReplyPreview {
                agent: address.into(),
                after: session.current_cursor().ok()?,
                reply: Box::new(reply.clone()),
            })
        });
        (live, preview)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{EventPayload, EventStore, Record, ReplyPreview, TurnIndex, REPLY_PREVIEW_BYTES};

    #[test]
    fn live_reply_retains_a_utf8_prefix_and_stops_at_the_first_missing_byte() {
        let mut reply = ReplyPreview::new(3, 4);
        reply.append(&"x".repeat(REPLY_PREVIEW_BYTES - 1));
        reply.append("界");
        reply.append("a");
        assert_eq!(REPLY_PREVIEW_BYTES - 1, reply.text.len());
        assert!(reply.text.ends_with('x'));
        assert!(reply.truncated);
    }

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
