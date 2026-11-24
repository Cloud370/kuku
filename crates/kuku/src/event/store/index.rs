use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use rpds::HashTrieMapSync;
use serde::{Deserialize, Serialize};

use super::message_index::MessageIndex;
use super::{EventOffset, EventPayload, StoredEvent};
use crate::event::history::{hidden_history, hidden_range};
use crate::{Error, Result};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct SegmentMark {
    pub(crate) id: u64,
    pub(crate) turn: Option<u64>,
    confirmed: u64,
    checkpoint: bool,
}

#[derive(Default)]
pub(crate) struct ContextMarks {
    pub(crate) segment: Option<SegmentMark>,
    pub(crate) checkpoint: Option<u64>,
    pub(crate) configurations: BTreeMap<u64, RouteSelectors>,
    pub(crate) hidden: Vec<Range<u64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct RouteSelectors {
    pub(crate) query_connection: Option<String>,
    pub(crate) query_model: Option<String>,
}

impl RouteSelectors {
    pub(crate) fn from_agent_config(config: &serde_json::Value) -> Self {
        Self {
            query_connection: config["connection"].as_str().map(str::to_owned),
            query_model: config["model"].as_str().map(str::to_owned),
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct FileFact {
    pub(super) location: EventOffset,
    pub(super) bytes: usize,
}

#[derive(Default)]
pub(crate) struct HistoryMarks {
    pub(crate) cursor: u64,
    pub(crate) controls: Vec<StoredEvent>,
    pub(crate) turn_owners: TurnOwnerIndex,
    pub(crate) message_rewinds: Vec<MessageRewind>,
    pub(crate) editable: HashMap<String, super::MessagePreviews>,
}

#[derive(Clone, Debug)]
pub struct MessageRewind {
    pub event_id: u64,
    pub target_event_id: u64,
    pub agent: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TurnOwners {
    pub(crate) first: String,
    pub(crate) last: String,
}

pub(crate) type TurnOwnerIndex = HashTrieMapSync<u64, TurnOwners>;

#[derive(Default, Clone, Serialize, Deserialize)]
pub(crate) struct JournalSummary {
    pub(crate) title: Option<String>,
    pub(crate) created_at: String,
    pub(crate) turns: u64,
    pub(crate) main_completed: bool,
}

impl JournalSummary {
    pub(super) fn observe(&mut self, event: &StoredEvent) {
        match &event.payload {
            EventPayload::TurnStarted { conversation, .. } => {
                self.turns += 1;
                if conversation == "main" {
                    self.main_completed = false;
                }
            }
            EventPayload::TurnCompleted { conversation, .. }
            | EventPayload::TurnCancelled { conversation, .. }
            | EventPayload::TurnInterrupted { conversation, .. }
                if conversation == "main" =>
            {
                self.main_completed = matches!(event.payload, EventPayload::TurnCompleted { .. });
            }
            EventPayload::MessageUser {
                text,
                job_delivery: false,
                ..
            } if self.title.is_none() => {
                self.title = Some(text.clone());
            }
            EventPayload::SessionCreated { created_at, .. } => {
                self.created_at.clone_from(created_at)
            }
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["operation"] == "session_claimed"
                    && self.created_at.is_empty() =>
            {
                if let Some(created_at) = value["record"]["claim"]["created_at"].as_str() {
                    self.created_at = created_at.into();
                }
            }
            _ => {}
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct JournalIndex {
    pub(super) summary: JournalSummary,
    agents: HashMap<String, AgentFacts>,
    rewinds: Vec<StoredEvent>,
    turn_owners: TurnOwnerIndex,
    pub(super) files: Vec<FileFact>,
}

#[derive(Default, Serialize, Deserialize)]
struct AgentFacts {
    turns: HashMap<u64, (bool, Option<u64>)>,
    messages: MessageIndex,
    admissions: Vec<u64>,
    turn: Option<u64>,
    #[serde(with = "crate::event::checkpoint_map")]
    configurations: HashMap<RouteSelectors, u64>,
    candidate: Option<SegmentMark>,
    confirmed_segments: Vec<SegmentMark>,
    compaction_reverts: Vec<(u64, u64)>,
    invalid_rewind: Option<String>,
}

impl JournalIndex {
    pub(super) fn observe_location(&mut self, event: &StoredEvent, offset: u64, bytes: usize) {
        self.observe(event);
        if let EventPayload::ToolResult {
            structured: Some(value),
            ..
        } = &event.payload
        {
            if matches!(
                value["kind"].as_str(),
                Some(
                    "file_content" | "file_edit" | "file_write" | "memory_write" | "forget_memory"
                )
            ) {
                self.files.push(FileFact {
                    location: EventOffset {
                        id: event.id,
                        offset,
                    },
                    bytes,
                });
            }
        }
    }

    pub(super) fn observe(&mut self, event: &StoredEvent) {
        use crate::event::history::{event_conversation, event_turn};
        self.summary.observe(event);
        if let (Some(turn), Some(owner)) = (
            event_turn(&event.payload),
            event_conversation(&event.payload),
        ) {
            match self.turn_owners.get(&turn) {
                Some(owners) if owners.last == owner => {}
                Some(owners) => {
                    let first = owners.first.clone();
                    self.turn_owners.insert_mut(
                        turn,
                        TurnOwners {
                            first,
                            last: owner.into(),
                        },
                    );
                }
                None => self.turn_owners.insert_mut(
                    turn,
                    TurnOwners {
                        first: owner.into(),
                        last: owner.into(),
                    },
                ),
            }
        }
        match &event.payload {
            EventPayload::TurnStarted {
                conversation, turn, ..
            } => {
                self.agent(conversation).turn = Some(*turn);
            }
            EventPayload::TurnCompleted {
                conversation, turn, ..
            }
            | EventPayload::TurnCancelled {
                conversation, turn, ..
            }
            | EventPayload::TurnInterrupted {
                conversation, turn, ..
            } => {
                self.agent(conversation).turns.entry(*turn).or_default().0 = true;
            }
            EventPayload::MessageUser {
                conversation,
                turn,
                text,
                job_delivery,
                ..
            } => {
                let preview = (!job_delivery).then(|| text.chars().take(160).collect());
                self.agent(conversation)
                    .messages
                    .push(event.id, *turn, preview);
            }
            EventPayload::Unknown(value) => self.observe_runtime(event.id, value),
            _ => {}
        }
    }

    pub(super) fn last_turn_owners(&self) -> HashMap<u64, String> {
        self.turn_owners
            .iter()
            .map(|(turn, owners)| (*turn, owners.last.clone()))
            .collect()
    }

    pub(super) fn history_marks(
        &self,
        include_owners: bool,
        include_previews: bool,
        cursor: u64,
    ) -> HistoryMarks {
        HistoryMarks {
            cursor,
            controls: self.rewind_controls(),
            turn_owners: if include_owners || !self.rewinds.is_empty() {
                self.turn_owners.clone()
            } else {
                TurnOwnerIndex::default()
            },
            message_rewinds: self
                .rewinds
                .iter()
                .filter_map(|event| {
                    let EventPayload::Unknown(value) = &event.payload else {
                        return None;
                    };
                    let record = &value["record"];
                    if record["operation"] != "message_rewind" {
                        return None;
                    }
                    let agent = record["address"].as_str()?;
                    let target_event_id = record["target_event_id"].as_u64()?;
                    (target_event_id < event.id
                        && self.agents.get(agent)?.messages.contains(target_event_id))
                    .then(|| MessageRewind {
                        event_id: event.id,
                        target_event_id,
                        agent: agent.into(),
                    })
                })
                .collect(),
            editable: if include_previews {
                self.agents
                    .iter()
                    .map(|(address, facts)| (address.clone(), facts.messages.previews.clone()))
                    .collect()
            } else {
                HashMap::new()
            },
        }
    }

    pub(super) fn rewind_start(&self, address: &str, target: u64) -> u64 {
        let Some(agent) = self.agents.get(address) else {
            return target.saturating_sub(1);
        };
        let last_before = |ids: &[u64]| {
            ids[..ids.partition_point(|id| *id < target)]
                .last()
                .copied()
        };
        agent.messages.latest_before(target).max(
            last_before(&agent.admissions)
                .unwrap_or_default()
                .saturating_sub(1),
        )
    }

    pub(super) fn rewind_controls(&self) -> Vec<StoredEvent> {
        self.rewinds.clone()
    }

    pub(super) fn turn_terminal(&self, address: &str, turn: u64) -> (bool, Option<u64>) {
        self.agents
            .get(address)
            .and_then(|agent| agent.turns.get(&turn))
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn context_marks(&self, address: &str) -> Result<ContextMarks> {
        let Some(agent) = self.agents.get(address) else {
            return Ok(ContextMarks::default());
        };
        if let Some(error) = &agent.invalid_rewind {
            return Err(Error::InvalidEventStream(error.clone()));
        }
        let hidden = hidden_history(self.rewinds.iter(), address);
        let mut segments = agent.confirmed_segments.iter().rev().filter(|segment| {
            hidden_range(&hidden, segment.id).is_none()
                && hidden_range(&hidden, segment.confirmed).is_none()
                && !agent.compaction_reverts.iter().any(|&(at, first)| {
                    first <= segment.id
                        && segment.confirmed < at
                        && hidden_range(&hidden, at).is_none()
                })
        });
        let segment = segments.next().copied();
        let checkpoint = segment
            .filter(|mark| mark.checkpoint)
            .or_else(|| segments.find(|mark| mark.checkpoint).copied())
            .map(|mark| mark.id);
        // Rewinds can keep admissions recorded later for messages that survived the cut.
        let jumps = self.rewinds.iter().filter(|event| {
            matches!(&event.payload, EventPayload::Unknown(value)
                if value["record"]["operation"] == "history_jump")
        });
        Ok(ContextMarks {
            segment,
            checkpoint,
            hidden: hidden_history(jumps, address),
            configurations: agent
                .configurations
                .iter()
                .map(|(selectors, id)| (*id, selectors.clone()))
                .collect(),
        })
    }

    fn agent(&mut self, address: &str) -> &mut AgentFacts {
        if !self.agents.contains_key(address) {
            self.agents.insert(address.into(), AgentFacts::default());
        }
        self.agents.get_mut(address).unwrap()
    }

    fn observe_runtime(&mut self, id: u64, value: &serde_json::Value) {
        match value["kind"].as_str() {
            Some("runtime.operation") => self.observe_operation(id, &value["record"]),
            Some("runtime.context_segment") => {
                if let Some(address) = value["agent"].as_str() {
                    let agent = self.agent(address);
                    agent.candidate = Some(SegmentMark {
                        id,
                        turn: agent.turn,
                        confirmed: 0,
                        checkpoint: false,
                    });
                }
            }
            Some("runtime.step_prepared" | "runtime.compaction_finished") => {
                let checkpoint = value["kind"] == "runtime.compaction_finished";
                if checkpoint && value["status"] != "completed" {
                    return;
                }
                if let Some(agent) = value["agent"]
                    .as_str()
                    .and_then(|address| self.agents.get_mut(address))
                {
                    if let Some(mut candidate) = agent
                        .candidate
                        .filter(|segment| value["segment"] == segment.id)
                    {
                        candidate.checkpoint = checkpoint;
                        candidate.confirmed = id;
                        agent.confirmed_segments.push(candidate);
                        agent.candidate = None;
                    }
                }
            }
            _ => {}
        }
    }

    fn observe_operation(&mut self, id: u64, record: &serde_json::Value) {
        let Some(operation) = record["operation"].as_str() else {
            return;
        };
        if operation == "input_delivered" && record["config"].is_object() {
            if let Some(address) = record["input"]["target"].as_str() {
                self.agent(address)
                    .remember_configuration(id, &record["config"]);
            }
            return;
        }
        let Some(address) = record["address"].as_str() else {
            return;
        };
        match operation {
            "agent_created" | "agent_configured" => {
                self.agent(address)
                    .remember_configuration(id, &record["config"]);
            }
            "agent_stopped" => {
                if let Some(turn) = record["interruption"]["turn"].as_u64() {
                    let facts = self.agent(address).turns.entry(turn).or_default();
                    if !facts.0 {
                        facts.1 = Some(id);
                    }
                }
            }
            "inputs_accepted" => self.agent(address).admissions.push(id),
            "message_rewind" | "history_jump" => {
                let target = record["target_event_id"].as_u64();
                let agent = self.agent(address);
                agent.discard_candidate(target, "history change has no target event");
                if let Some(target_event_id) = target {
                    self.rewinds.push(StoredEvent {
                        id,
                        payload: EventPayload::Unknown(serde_json::json!({
                            "id":id,"kind":"runtime.operation","record":{
                                "operation":operation,"address":address,"target_event_id":target_event_id
                            }
                        })),
                    });
                }
            }
            "compaction_reverted" => {
                let agent = self.agent(address);
                let target = record["segment"].as_u64();
                agent.discard_candidate(target, "compaction rewind has no segment");
                if let Some(target) = target {
                    agent.compaction_reverts.push((id, target));
                }
            }
            _ => {}
        }
    }
}

impl AgentFacts {
    fn remember_configuration(&mut self, id: u64, config: &serde_json::Value) {
        self.configurations
            .insert(RouteSelectors::from_agent_config(config), id);
    }

    fn discard_candidate(&mut self, target: Option<u64>, error: &'static str) {
        let Some(target) = target else {
            self.invalid_rewind.get_or_insert_with(|| error.into());
            return;
        };
        if self.candidate.is_some_and(|segment| segment.id >= target) {
            self.candidate = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{JournalIndex, StoredEvent};

    fn message(id: u64, turn: u64, agent: &str, text: &str, job_delivery: bool) -> StoredEvent {
        serde_json::from_value(json!({
            "id":id,"kind":"message.user","ts":"","conversation":agent,
            "turn":turn,"text":text,"job_delivery":job_delivery
        }))
        .unwrap()
    }

    #[test]
    fn history_views_share_index_entries_across_reads_and_committed_updates() {
        let mut index = JournalIndex::default();
        index.observe(&message(1, 1, "main", "First message", false));
        index.observe(&message(2, 2, "main", "Second message", false));
        index.observe(&message(3, 2, "main", "Job output", true));
        let old = index.history_marks(true, true, 3);
        let same = index.history_marks(true, true, 3);
        assert!(old.turn_owners.ptr_eq(&same.turn_owners));
        assert!(old.editable["main"].ptr_eq(&same.editable["main"]));

        index.observe(&message(4, 2, "main", "Follow-up", false));
        index.observe(&message(5, 2, "peer", "Peer message", false));
        let current = index.history_marks(true, true, 5);
        assert_eq!("main", old.turn_owners[&2].last);
        assert_eq!("main", current.turn_owners[&2].first);
        assert_eq!("peer", current.turn_owners[&2].last);
        assert_eq!(1, old.editable["main"][&2].len());
        assert_eq!(2, current.editable["main"][&2].len());
        assert!(std::ptr::eq(
            &old.editable["main"][&1][0],
            &current.editable["main"][&1][0]
        ));

        let saved = serde_json::to_value(&index).unwrap();
        assert_eq!(
            json!([
                {"id":1,"turn":1,"preview":"First message"},
                {"id":2,"turn":2,"preview":"Second message"},
                {"id":3,"turn":2,"preview":null},
                {"id":4,"turn":2,"preview":"Follow-up"}
            ]),
            saved["agents"]["main"]["messages"]
        );
        let restored: JournalIndex = serde_json::from_value(saved).unwrap();
        let history = restored.history_marks(true, true, 5);
        assert_eq!("Follow-up", history.editable["main"][&2][1].text);
        assert!(restored.agents["main"].messages.contains(3));
    }
}
