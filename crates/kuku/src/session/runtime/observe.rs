use std::collections::{HashMap, VecDeque};
use std::path::Path;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::Result;
use crate::event::{EventStore, StoredEvent};
use crate::query::{ModelPhase, UiEvent};

use super::receipts::validate_key;
use super::record::{Record, State};
use super::{
    AgentSnapshot, AgentStatus, AgentView, InputStatus, PendingInput, Receipt, Session,
    TurnSummary, load_state,
};

use crate::event::REPLY_PREFIX_BYTES as REPLY_PREVIEW_BYTES;
const PEER_PREVIEW_BYTES: usize = 512;

/// Observed phase and visible reply prefix of the current model request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplyPreview {
    /// Execution owning the current model request.
    pub execution_id: u64,
    /// Turn of the active model request.
    pub turn: u64,
    /// Most recent provider activity at the observation boundary.
    pub phase: ModelPhase,
    /// Tool argument activity is known to be preparing file contents.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub preparing_file_contents: bool,
    /// UTF-8 prefix from the start of this request's reply.
    pub text: String,
    /// Public reasoning prefix for the selected Agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reasoning_preview: String,
    /// Additional public summary text exists beyond the retained prefix.
    pub reasoning_preview_truncated: bool,
    /// The reply already exceeded the preview's byte limit.
    pub truncated: bool,
    /// Bytes of visible output, including text beyond the retained prefix.
    pub output_bytes: u64,
    /// Latest output count this request received from the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

impl ReplyPreview {
    fn new(execution_id: u64, turn: u64, phase: ModelPhase) -> Self {
        Self {
            execution_id,
            turn,
            phase,
            preparing_file_contents: false,
            text: String::new(),
            reasoning_preview: String::new(),
            reasoning_preview_truncated: false,
            truncated: false,
            output_bytes: 0,
            output_tokens: None,
        }
    }

    fn append(&mut self, text: &str) {
        self.output_bytes = self.output_bytes.saturating_add(text.len() as u64);
        append_prefix(&mut self.text, &mut self.truncated, text);
    }

    fn excerpt(&self) -> Self {
        let end = self
            .text
            .floor_char_boundary(self.text.len().min(PEER_PREVIEW_BYTES));
        Self {
            text: self.text[..end].to_owned(),
            reasoning_preview: String::new(),
            reasoning_preview_truncated: false,
            truncated: self.truncated || end < self.text.len(),
            ..*self
        }
    }
}

fn append_prefix(prefix: &mut String, truncated: &mut bool, text: &str) {
    if *truncated {
        return;
    }
    let end = text.floor_char_boundary(text.len().min(REPLY_PREVIEW_BYTES - prefix.len()));
    prefix.push_str(&text[..end]);
    *truncated = end < text.len();
}

/// Active reply previews captured with one Session cursor.
#[derive(Debug, Default)]
pub struct ReplyPreviews {
    /// The selected Agent's active reply.
    pub selected: Option<ReplyPreview>,
    /// Phases and shorter excerpts for peer Agents, indexed by address.
    pub peers: HashMap<String, ReplyPreview>,
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
    preview_scope: PreviewScope,
    previews: VecDeque<Observation>,
}

enum PreviewScope {
    None,
    Selected(String),
    WithActivity(String),
}

impl Session {
    /// Follow the selected Agent's complete reply preview and bounded prefixes from active peers.
    pub fn observe_agent_with_activity(&self, after: u64, address: &str) -> SessionObserver {
        SessionObserver::with_scope(
            self.clone(),
            after,
            PreviewScope::WithActivity(address.to_owned()),
        )
    }

    /// Return the one-based display number for a committed Agent Turn identity.
    pub fn turn_number(&self, address: &str, turn: u64) -> Option<u64> {
        self.inner.state.lock().unwrap().turns.number(address, turn)
    }

    /// Return the Agent's conversation points across all branches, newest first.
    pub fn history_points(
        &self,
        address: &str,
        before: u64,
        limit: usize,
    ) -> Vec<super::HistoryPoint> {
        self.inner
            .state
            .lock()
            .unwrap()
            .turns
            .history_points(address, before, limit)
    }

    /// Return the durable first-admission time for a specific Agent Turn.
    pub fn turn_started_at_ms(&self, address: &str, turn: u64) -> Option<u64> {
        self.inner
            .state
            .lock()
            .unwrap()
            .turns
            .started_at_ms(address, turn)
    }

    /// Find the statistics anchored to a committed terminal event.
    pub fn turn_summary_at(&self, event_id: u64) -> Option<(u64, TurnSummary)> {
        self.inner.state.lock().unwrap().turns.summary_at(event_id)
    }

    /// Return whether an explicit stop ended the selected Turn.
    pub fn turn_explicitly_stopped(&self, address: &str, turn: u64) -> bool {
        self.inner
            .state
            .lock()
            .unwrap()
            .turns
            .explicitly_stopped(address, turn)
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
                    Some((peer.clone(), reply_preview(&state, peer)?.excerpt()))
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
        Ok((state.snapshot(address)?, cursor))
    }

    /// Return retained Agent snapshots and their committed cursor under one state lock.
    pub fn agents_with_cursor(&self) -> Result<(Vec<AgentSnapshot>, u64)> {
        let state = self.inner.state.lock().unwrap();
        let cursor = if state.closed {
            state.closed_cursor
        } else {
            self.current_cursor()?
        };
        Ok((state.snapshots(), cursor))
    }

    /// Read a closed session's selected snapshot and pending inputs without opening runtime resources.
    pub fn read_view(events_path: &Path, address: &str) -> Result<AgentView> {
        let (mut state, cursor) = load_state(events_path)?;
        super::controls::refresh_cancelled_turn_blocker(
            events_path,
            address,
            state.agent_mut(address)?,
            cursor,
        )?;
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

    pub(crate) async fn wait_for_delivery(
        &self,
        sender: &str,
        address: &str,
        event_id: u64,
        timeout: std::time::Duration,
    ) -> Result<serde_json::Value> {
        let changed = EventStore::open(self.events_path())?.changed();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut timed_out = false;
        loop {
            let notified = changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (state, agent_status, turn, settlement) = {
                let state = self.inner.state.lock().unwrap();
                let delivery = state
                    .delegated_deliveries
                    .get(&event_id)
                    .filter(|delivery| delivery.sender == sender)
                    .ok_or_else(|| {
                        crate::Error::InvalidArgument(format!(
                            "no message delivery receipt {event_id} from '{sender}'"
                        ))
                    })?;
                let input = state.input_status(address, &delivery.command_id)?;
                let agent_status = state.agent(address)?.snapshot.status;
                let turn = delivery.turn;
                let settlement = turn.and_then(|turn| state.turns.settlement(address, turn));
                (input, agent_status, turn, settlement)
            };
            let mut result = serde_json::json!({
                "address": address,
                "receipt": event_id,
                "input_status": state,
            });
            if let Some(turn) = turn {
                result["turn"] = turn.into();
            }
            if let Some((terminal, summary)) = settlement {
                let mut events = EventStore::read_page(self.events_path(), terminal - 1, 1)?;
                let record = events
                    .pop()
                    .filter(|event| event.id == terminal)
                    .map(|event| Record::from_event(&event))
                    .transpose()?
                    .flatten();
                match record {
                    Some(Record::TurnSettled {
                        status,
                        output,
                        error,
                        ..
                    }) => {
                        result["status"] = if status == AgentStatus::Idle {
                            "completed".into()
                        } else {
                            serde_json::json!(status)
                        };
                        if let Some(output) = output {
                            result["output"] = output.into();
                        }
                        if let Some(error) = error {
                            result["error"] = error.into();
                        }
                    }
                    Some(Record::AgentStopped { .. }) => result["status"] = "stopped".into(),
                    _ => {
                        return Err(crate::Error::InvalidEventStream(format!(
                            "turn settlement {terminal} is missing"
                        )));
                    }
                }
                result["settlement"] = terminal.into();
                result["summary"] = serde_json::to_value(summary)?;
                return Ok(result);
            }
            result["agent_status"] = serde_json::json!(agent_status);
            if state == InputStatus::Withdrawn {
                result["status"] = "withdrawn".into();
                return Ok(result);
            }
            if agent_status != AgentStatus::Running {
                result["status"] = "pending".into();
                return Ok(result);
            }
            if timed_out {
                result["status"] = "timed_out".into();
                return Ok(result);
            }
            timed_out = tokio::time::timeout_at(deadline, notified).await.is_err();
        }
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

    pub(super) fn publish_locked(
        &self,
        state: &mut State,
        address: &str,
        execution_id: u64,
        turn: u64,
        event: UiEvent,
    ) {
        if matches!(
            event,
            UiEvent::TextDelta { .. }
                | UiEvent::ThinkingDelta { .. }
                | UiEvent::ModelProgress { .. }
                | UiEvent::ModelOutputUsage { .. }
                | UiEvent::ModelRequest { .. }
                | UiEvent::ModelRecovery { .. }
        ) {
            let Some(agent) = state.agents.get_mut(address).filter(|agent| {
                agent.snapshot.status == AgentStatus::Running
                    && agent.snapshot.execution_id == Some(execution_id)
                    && agent.snapshot.turn == Some(turn)
            }) else {
                return;
            };
            let phase = match &event {
                UiEvent::TextDelta { text } if !text.is_empty() => Some(ModelPhase::Streaming),
                UiEvent::ThinkingDelta { text } if !text.is_empty() => Some(ModelPhase::Thinking),
                UiEvent::ModelProgress { phase, .. } => Some(*phase),
                UiEvent::ModelOutputUsage { output_tokens } => {
                    if let Some(preview) = &mut agent.reply_preview {
                        preview.output_tokens = Some(*output_tokens);
                    }
                    None
                }
                UiEvent::ModelRequest { .. } | UiEvent::ModelRecovery { .. } => {
                    agent.reply_preview = None;
                    None
                }
                _ => None,
            };
            if let Some(phase) = phase {
                let preview = agent
                    .reply_preview
                    .get_or_insert_with(|| ReplyPreview::new(execution_id, turn, phase));
                preview.phase = phase;
                preview.preparing_file_contents = matches!(
                    &event,
                    UiEvent::ModelProgress {
                        preparing_file_contents: true,
                        ..
                    }
                );
                if let UiEvent::TextDelta { text } = &event {
                    preview.append(text);
                } else if let UiEvent::ThinkingDelta { text } = &event {
                    append_prefix(
                        &mut preview.reasoning_preview,
                        &mut preview.reasoning_preview_truncated,
                        text,
                    );
                }
            }
        }
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
        if let Some(preview) = self.previews.pop_front() {
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
            if let Some(preview) = self.previews.pop_front() {
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
        let scope = preview_agent.map_or(PreviewScope::None, PreviewScope::Selected);
        Self::with_scope(session, cursor, scope)
    }

    fn with_scope(session: Session, cursor: u64, preview_scope: PreviewScope) -> Self {
        let (live, previews) = Self::attach(&session, &preview_scope);
        Self {
            session,
            cursor,
            page: VecDeque::new(),
            live,
            preview_scope,
            previews,
        }
    }

    fn reattach(&mut self) {
        if !matches!(self.preview_scope, PreviewScope::None) {
            (self.live, self.previews) = Self::attach(&self.session, &self.preview_scope);
        }
    }

    fn attach(
        session: &Session,
        scope: &PreviewScope,
    ) -> (broadcast::Receiver<LiveObservation>, VecDeque<Observation>) {
        let (address, include_peers) = match scope {
            PreviewScope::None => return (session.inner.live.subscribe(), VecDeque::new()),
            PreviewScope::Selected(address) => (address, false),
            PreviewScope::WithActivity(address) => (address, true),
        };
        let state = session.inner.state.lock().unwrap();
        let live = session.inner.live.subscribe();
        let mut previews = VecDeque::new();
        let Ok(after) = session.current_cursor() else {
            return (live, previews);
        };
        if let Some(reply) = reply_preview(&state, address) {
            previews.push_back(Observation::ReplyPreview {
                agent: address.clone(),
                after,
                reply: Box::new(reply.clone()),
            });
        }
        if include_peers {
            previews.extend(
                state
                    .agents
                    .keys()
                    .filter(|peer| *peer != address)
                    .filter_map(|peer| {
                        let reply = reply_preview(&state, peer)?;
                        Some(Observation::ReplyPreview {
                            agent: peer.clone(),
                            after,
                            reply: Box::new(reply.excerpt()),
                        })
                    }),
            );
        }
        (live, previews)
    }
}

#[cfg(test)]
mod tests {
    use crate::query::ModelPhase;
    use serde_json::json;

    use super::super::{TurnIndex, record::Record};
    use super::{EventStore, REPLY_PREVIEW_BYTES, ReplyPreview};
    use crate::event::EventPayload;

    #[test]
    fn live_reply_retains_a_utf8_prefix_and_stops_at_the_first_missing_byte() {
        let mut reply = ReplyPreview::new(3, 4, ModelPhase::Streaming);
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
                    message_input_ids: Vec::new(),
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
