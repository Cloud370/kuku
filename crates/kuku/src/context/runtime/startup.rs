use std::collections::BTreeMap;
use std::path::Path;

use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::{Error, Result};

use super::{MAX_SELECTION_BYTES_FACTOR, SnapshotCache, belongs};

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct StartupCache {
    after: u64,
    budget: usize,
    session: Option<StoredEvent>,
    agents: BTreeMap<String, AgentStartup>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AgentStartup {
    selection: Selection,
    failure: Option<String>,
}

impl StartupCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            after: 0,
            budget: max_bytes.saturating_mul(MAX_SELECTION_BYTES_FACTOR),
            session: None,
            agents: BTreeMap::new(),
        }
    }

    pub(crate) fn at_limit(self, max_bytes: usize) -> Self {
        if self.accepts_limit(max_bytes) {
            self
        } else {
            Self::new(max_bytes)
        }
    }

    pub(crate) fn accepts_limit(&self, max_bytes: usize) -> bool {
        self.budget == max_bytes.saturating_mul(MAX_SELECTION_BYTES_FACTOR)
    }

    pub(crate) fn clear(&mut self) {
        self.session = None;
        self.agents.clear();
    }

    pub(crate) fn observe(&mut self, event: &StoredEvent) {
        self.after = event.id;
        if matches!(&event.payload, EventPayload::SessionCreated { .. }) {
            self.session = Some(event.clone());
            for (address, startup) in &mut self.agents {
                startup.observe(event, address);
            }
            return;
        }
        match startup_owner(&event.payload) {
            Some(StartupOwner::Agent(address)) => {
                let startup = self.agents.entry(address.into()).or_insert_with(|| {
                    let mut startup = AgentStartup::new(self.budget);
                    if let Some(session) = &self.session {
                        startup.observe(session, address);
                    }
                    startup
                });
                startup.observe(event, address);
            }
            Some(StartupOwner::Turn(turn)) => {
                for (address, startup) in &mut self.agents {
                    if startup.selection.latest_turn == Some(turn) {
                        startup.observe(event, address);
                    }
                }
            }
            None => {}
        }
    }

    pub(crate) fn read(&mut self, path: &Path, agent: &str) -> Result<Vec<StoredEvent>> {
        EventStore::visit_after_path(path, self.after, |event| {
            self.observe(&event);
            Ok(())
        })?;
        if let Some(startup) = self.agents.get_mut(agent) {
            if let Some(failure) = &startup.failure {
                return Err(Error::InvalidArgument(failure.clone()));
            }
            return startup.selection.events();
        }
        Ok(self.session.iter().cloned().collect())
    }
}

impl AgentStartup {
    fn new(budget: usize) -> Self {
        Self {
            selection: Selection::new(budget),
            failure: None,
        }
    }

    fn observe(&mut self, event: &StoredEvent, address: &str) {
        if self.failure.is_none() {
            if let Err(error) = self.selection.observe(event.clone(), address) {
                self.failure = Some(match error {
                    Error::InvalidArgument(message) => message,
                    error => error.to_string(),
                });
                self.selection = Selection::new(self.selection.budget);
            }
        }
    }
}

enum StartupOwner<'a> {
    Agent(&'a str),
    Turn(u64),
}

fn startup_owner(payload: &EventPayload) -> Option<StartupOwner<'_>> {
    match payload {
        EventPayload::PromptSnapshot { conversation, .. }
        | EventPayload::ContextSkills { conversation, .. }
        | EventPayload::MessageUser { conversation, .. }
        | EventPayload::TurnStarted { conversation, .. }
        | EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. }
        | EventPayload::ModelRecovery { conversation, .. } => {
            Some(StartupOwner::Agent(conversation))
        }
        EventPayload::ModelResponse {
            turn, conversation, ..
        }
        | EventPayload::ModelError {
            turn, conversation, ..
        }
        | EventPayload::ToolCall {
            turn, conversation, ..
        }
        | EventPayload::ToolResult {
            turn, conversation, ..
        } => Some(
            conversation
                .as_deref()
                .map_or(StartupOwner::Turn(*turn), StartupOwner::Agent),
        ),
        EventPayload::PermissionRequested { turn, .. }
        | EventPayload::PermissionAllow { turn, .. }
        | EventPayload::PermissionDeny { turn, .. } => Some(StartupOwner::Turn(*turn)),
        EventPayload::Unknown(value) => {
            let record = &value["record"];
            match value["kind"].as_str() {
                Some("runtime.operation")
                    if matches!(
                        record["operation"].as_str(),
                        Some("agent_created" | "agent_configured")
                    ) =>
                {
                    record["address"].as_str().map(StartupOwner::Agent)
                }
                Some(
                    "runtime.context_admitted"
                    | "runtime.step_prepared"
                    | "runtime.tool_prepared"
                    | "runtime.tool_dispatched"
                    | "runtime.model_rejected"
                    | "runtime.compaction_started"
                    | "runtime.model_retry",
                ) => value["agent"].as_str().map(StartupOwner::Agent),
                _ => None,
            }
        }
        _ => None,
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Selection {
    latest_turn: Option<u64>,
    metadata: BTreeMap<String, StoredEvent>,
    current: Vec<StoredEvent>,
    skills: Vec<StoredEvent>,
    snapshots: SnapshotCache,
    metadata_bytes: usize,
    current_bytes: usize,
    skills_bytes: usize,
    budget: usize,
}

impl Selection {
    fn new(budget: usize) -> Self {
        Self {
            latest_turn: None,
            metadata: BTreeMap::new(),
            current: Vec::new(),
            skills: Vec::new(),
            snapshots: SnapshotCache::default(),
            metadata_bytes: 2,
            current_bytes: 2,
            skills_bytes: 2,
            budget,
        }
    }

    fn observe(&mut self, mut event: StoredEvent, agent: &str) -> Result<()> {
        let mut changed = false;
        let key = match &event.payload {
            EventPayload::SessionCreated { .. } => Some("session".into()),
            EventPayload::PromptSnapshot { conversation, .. } if conversation == agent => {
                Some("prompt".into())
            }
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["address"] == agent
                    && matches!(
                        value["record"]["operation"].as_str(),
                        Some("agent_created" | "agent_configured")
                    ) =>
            {
                value["record"]["operation"].as_str().map(str::to_string)
            }
            _ => None,
        };
        if let Some(key) = key {
            self.metadata.insert(key, event);
            self.metadata_bytes = serde_json::to_vec(&self.metadata)?.len();
            return Ok(());
        }
        if let EventPayload::ContextSkills { conversation, .. } = &event.payload {
            if conversation == agent {
                if self.skills.len() == 2 {
                    self.skills.remove(0);
                }
                self.skills.push(event);
                self.skills_bytes = serde_json::to_vec(&self.skills)?.len();
            }
            return Ok(());
        }
        if let EventPayload::TurnStarted {
            turn, conversation, ..
        } = &event.payload
        {
            if conversation == agent {
                self.latest_turn = Some(*turn);
                self.current.clear();
                self.current_bytes = json_array_bytes(1, serde_json::to_vec(&event)?.len());
                self.current.push(event);
            }
            return Ok(());
        }
        if let EventPayload::Unknown(value) = &event.payload {
            if value["agent"] == agent {
                self.snapshots.observe_context(value);
            }
        }
        changed |= self
            .snapshots
            .observe_result(&mut event, agent, self.latest_turn)?;
        let turn = match &event.payload {
            EventPayload::ModelResponse {
                turn, conversation, ..
            }
            | EventPayload::ModelError {
                turn, conversation, ..
            }
            | EventPayload::ToolCall {
                turn, conversation, ..
            }
            | EventPayload::ToolResult {
                turn, conversation, ..
            } if belongs(conversation.as_deref(), *turn, agent, self.latest_turn) => Some(*turn),
            EventPayload::PermissionRequested { turn, .. }
            | EventPayload::PermissionAllow { turn, .. }
            | EventPayload::PermissionDeny { turn, .. } => Some(*turn),
            EventPayload::ModelRecovery {
                turn, conversation, ..
            } if conversation == agent => Some(*turn),
            EventPayload::TurnCompleted {
                turn, conversation, ..
            }
            | EventPayload::TurnCancelled {
                turn, conversation, ..
            }
            | EventPayload::TurnInterrupted {
                turn, conversation, ..
            }
            | EventPayload::MessageUser {
                turn, conversation, ..
            } if conversation == agent => Some(*turn),
            EventPayload::Unknown(value)
                if value["agent"] == agent
                    && matches!(
                        value["kind"].as_str(),
                        Some(
                            "runtime.tool_prepared"
                                | "runtime.tool_dispatched"
                                | "runtime.model_rejected"
                                | "runtime.compaction_started"
                        )
                    ) =>
            {
                value["turn"].as_u64()
            }
            EventPayload::Unknown(value)
                if value["agent"] == agent
                    && value["kind"] == "runtime.step_prepared"
                    && value["input_skills"]
                        .as_array()
                        .is_some_and(|skills| !skills.is_empty()) =>
            {
                value["turn"].as_u64()
            }
            EventPayload::Unknown(value)
                if value["agent"] == agent
                    && value["kind"] == "runtime.model_retry"
                    && value["recovery"] == "regenerate_tool_arguments" =>
            {
                value["turn"].as_u64()
            }
            _ => None,
        };
        if turn.is_some() && turn == self.latest_turn {
            match &mut event.payload {
                EventPayload::ModelResponse {
                    conversation,
                    text,
                    thinking,
                    ..
                } => {
                    *conversation = Some(agent.into());
                    text.clear();
                    *thinking = None;
                }
                EventPayload::ModelError {
                    conversation,
                    message,
                    ..
                } => {
                    *conversation = Some(agent.into());
                    message.clear();
                }
                EventPayload::ToolCall { conversation, .. }
                | EventPayload::ToolResult { conversation, .. } => {
                    *conversation = Some(agent.into());
                }
                _ => {}
            }
            if let EventPayload::ToolResult {
                model_content,
                summary,
                structured,
                files_read,
                files_changed,
                commands_run,
                memory_changed,
                ..
            } = &mut event.payload
            {
                model_content.clear();
                summary.clear();
                *structured = structured.as_ref().and_then(|result| {
                    if result["kind"] != "skill_load" {
                        return None;
                    }
                    let name = result["name"].as_str().filter(|name| name.len() <= 64)?;
                    let version = result["version"]
                        .as_str()
                        .filter(|version| version.len() == 64)?;
                    Some(
                        serde_json::json!({"kind": "skill_load", "name": name, "version": version}),
                    )
                });
                files_read.clear();
                files_changed.clear();
                commands_run.clear();
                *memory_changed = None;
            }
            self.current_bytes +=
                serde_json::to_vec(&event)?.len() + usize::from(!self.current.is_empty());
            self.current.push(event);
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        trim_snapshot_cache(
            self.metadata_bytes + self.current_bytes + self.skills_bytes,
            &mut self.snapshots,
            self.budget,
        )?;
        Ok(())
    }

    fn events(&mut self) -> Result<Vec<StoredEvent>> {
        trim_snapshot_cache(
            self.metadata_bytes + self.current_bytes + self.skills_bytes,
            &mut self.snapshots,
            self.budget,
        )?;
        let selected = self
            .metadata
            .values()
            .chain(&self.current)
            .chain(&self.skills)
            .chain(
                self.snapshots
                    .entries
                    .iter()
                    .filter(|(key, snapshot)| {
                        matches!(key, super::SnapshotKey::Read(..))
                            || self.snapshots.visible_writes.contains(&snapshot.event.id)
                    })
                    .map(|(_, snapshot)| &snapshot.event),
            )
            .map(|event| (event.id, event))
            .collect::<BTreeMap<_, _>>();
        let result = selected.into_values().cloned().collect::<Vec<_>>();
        if serde_json::to_vec(&result)?.len() > self.budget {
            return Err(Error::InvalidArgument(
                "agent startup facts exceed the context selection byte limit".into(),
            ));
        }
        Ok(result)
    }
}

#[cfg(test)]
pub(super) fn read_agent_events(
    path: &Path,
    agent: &str,
    max_bytes: usize,
) -> Result<Vec<StoredEvent>> {
    StartupCache::new(max_bytes).read(path, agent)
}

fn trim_snapshot_cache(
    other_bytes: usize,
    snapshots: &mut SnapshotCache,
    budget: usize,
) -> Result<()> {
    loop {
        let entry_bytes = snapshots
            .entries
            .values()
            .map(|snapshot| {
                let access_bytes = snapshot.accessed.checked_ilog10().unwrap_or(0) as usize + 1;
                json_array_bytes(2, snapshot.event_bytes + access_bytes)
            })
            .sum();
        let snapshot_bytes = json_array_bytes(snapshots.entries.len(), entry_bytes);
        if json_array_bytes(4, other_bytes + snapshot_bytes) <= budget {
            return Ok(());
        }
        if !snapshots.remove_oldest() {
            return Err(Error::InvalidArgument(
                "agent startup facts exceed the context selection byte limit".into(),
            ));
        }
    }
}

fn json_array_bytes(entries: usize, content_bytes: usize) -> usize {
    content_bytes + entries.saturating_sub(1) + 2
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{EventPayload, SnapshotCache, StoredEvent, trim_snapshot_cache};
    use crate::context::runtime::{FileSnapshot, SnapshotKey};

    #[test]
    fn snapshot_budget_matches_the_encoded_selection_at_the_eviction_boundary() {
        let event = StoredEvent {
            id: 99,
            payload: EventPayload::Unknown(json!({"text":"a\\b\n文"})),
        };
        let accessed = 1000;
        let metadata = json!({"snapshot": "fixture"});
        let current = [&event];
        let skills = [&event];
        let exact_size = serde_json::to_vec(&(&metadata, current, skills, [(accessed, &event)]))
            .unwrap()
            .len();
        let other_bytes = serde_json::to_vec(&metadata).unwrap().len()
            + serde_json::to_vec(&current).unwrap().len()
            + serde_json::to_vec(&skills).unwrap().len();
        let mut snapshots = SnapshotCache::default();
        snapshots.entries.insert(
            SnapshotKey::Read("fixture".into(), 1),
            FileSnapshot {
                accessed,
                event_bytes: serde_json::to_vec(&event).unwrap().len(),
                event,
            },
        );
        trim_snapshot_cache(other_bytes, &mut snapshots, exact_size).unwrap();
        assert_eq!(1, snapshots.entries.len());
        trim_snapshot_cache(other_bytes, &mut snapshots, exact_size - 1).unwrap();
        assert!(snapshots.entries.is_empty());
    }
}
