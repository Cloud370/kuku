use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::{EventOffset, EventPayload, StoredEvent};
use crate::{Error, Result};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct SegmentMark {
    pub(crate) id: u64,
    pub(crate) turn: Option<u64>,
    checkpoint: bool,
}

#[derive(Default)]
pub(crate) struct ContextMarks {
    pub(crate) segment: Option<SegmentMark>,
    pub(crate) checkpoint: Option<u64>,
    pub(crate) configurations: BTreeMap<u64, RouteSelectors>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct RouteSelectors {
    pub(crate) query_connection: Option<String>,
    pub(crate) query_model: Option<String>,
    pub(crate) query_tier: Option<String>,
}

impl RouteSelectors {
    pub(crate) fn from_agent_config(config: &serde_json::Value) -> Self {
        Self {
            query_connection: config["connection"].as_str().map(str::to_owned),
            query_model: config["model"].as_str().map(str::to_owned),
            query_tier: config["tier"].as_str().map(str::to_owned),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct FileFact {
    pub(super) location: EventOffset,
    pub(super) bytes: usize,
}

#[derive(Default)]
pub(crate) struct HistoryMarks {
    pub(crate) controls: Vec<StoredEvent>,
    pub(crate) turn_owners: HashMap<u64, TurnOwners>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TurnOwners {
    pub(crate) first: String,
    pub(crate) last: String,
}

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
    turn_owners: HashMap<u64, TurnOwners>,
    pub(super) files: Vec<FileFact>,
}

#[derive(Default, Serialize, Deserialize)]
struct AgentFacts {
    turns: HashMap<u64, (bool, Option<u64>)>,
    messages: Vec<u64>,
    admissions: Vec<u64>,
    turn: Option<u64>,
    #[serde(with = "crate::event::checkpoint_map")]
    configurations: HashMap<RouteSelectors, u64>,
    candidate: Option<SegmentMark>,
    segments: Vec<SegmentMark>,
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
            self.turn_owners
                .entry(turn)
                .and_modify(|owners| {
                    if owners.last != owner {
                        owners.last = owner.into();
                    }
                })
                .or_insert_with(|| TurnOwners {
                    first: owner.into(),
                    last: owner.into(),
                });
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
            EventPayload::MessageUser { conversation, .. } => {
                self.agent(conversation).messages.push(event.id);
            }
            EventPayload::ConversationRollback { .. }
            | EventPayload::ConversationRollbackUndone { .. } => self.rewinds.push(event.clone()),
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

    pub(super) fn history_marks(&self, include_owners: bool) -> HistoryMarks {
        HistoryMarks {
            controls: self.rewind_controls(),
            turn_owners: if include_owners || !self.rewinds.is_empty() {
                self.turn_owners.clone()
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
        last_before(&agent.messages).unwrap_or_default().max(
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
        Ok(ContextMarks {
            segment: agent.segments.last().copied(),
            checkpoint: agent
                .segments
                .iter()
                .rev()
                .find(|segment| segment.checkpoint)
                .map(|segment| segment.id),
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
                        agent.segments.push(candidate);
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
            "message_rewind" => {
                let target = record["target_event_id"].as_u64();
                self.agent(address)
                    .rewind(target, "message rewind has no target event");
                if let Some(target_event_id) = target {
                    self.rewinds.push(StoredEvent {
                        id,
                        payload: EventPayload::Unknown(serde_json::json!({
                            "id":id,"kind":"runtime.operation","record":{
                                "operation":"message_rewind","address":address,"target_event_id":target_event_id
                            }
                        })),
                    });
                }
            }
            "compaction_reverted" => self.agent(address).rewind(
                record["segment"].as_u64(),
                "compaction rewind has no segment",
            ),
            _ => {}
        }
    }
}

impl AgentFacts {
    fn remember_configuration(&mut self, id: u64, config: &serde_json::Value) {
        self.configurations
            .insert(RouteSelectors::from_agent_config(config), id);
    }

    fn rewind(&mut self, target: Option<u64>, error: &'static str) {
        let Some(target) = target else {
            self.invalid_rewind.get_or_insert_with(|| error.into());
            return;
        };
        self.segments
            .truncate(self.segments.partition_point(|segment| segment.id < target));
        if self.candidate.is_some_and(|segment| segment.id >= target) {
            self.candidate = None;
        }
    }
}
