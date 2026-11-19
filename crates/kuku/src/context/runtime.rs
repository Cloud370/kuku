use std::collections::BTreeMap;
use std::path::Path;

use crate::context::{CanonicalMessage, MessageBlock, Role, ToolSchema};
use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::prompt::PromptCatalog;
use crate::{Error, Result};

const PAGE_RECORDS: usize = 256;
const MAX_SELECTION_BYTES_FACTOR: usize = 4;
const MAX_CACHED_READS: usize = 128;

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct FrozenSegment {
    #[serde(skip)]
    pub(crate) id: u64,
    pub(crate) signature: String,
    pub(crate) configuration: u64,
    pub(crate) handoff: u64,
    pub(crate) system_prompt: String,
    pub(crate) prelude: Vec<CanonicalMessage>,
    pub(crate) tools: Vec<ToolSchema>,
    pub(crate) catalog: PromptCatalog,
    #[serde(default)]
    pub(crate) connection: String,
    #[serde(default)]
    pub(crate) protocol: String,
    #[serde(default)]
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) endpoint: String,
    #[serde(default)]
    pub(crate) behavior: String,
    #[serde(default = "identity_slot")]
    pub(crate) identity_slot: usize,
    #[serde(default)]
    pub(crate) handoff_prelude: Option<usize>,
    #[serde(default)]
    pub(crate) workspace: String,
}

fn identity_slot() -> usize {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessageOrigin {
    Input,
    Response,
    ToolResults,
    Runtime,
}

impl MessageOrigin {
    fn of(message: &CanonicalMessage) -> Self {
        if message.role == Role::Assistant {
            Self::Response
        } else if message
            .blocks
            .iter()
            .any(|block| matches!(block, MessageBlock::ToolResult(_)))
        {
            Self::ToolResults
        } else {
            Self::Input
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AdmittedMessage {
    pub(crate) turn: u64,
    pub(crate) origin: MessageOrigin,
    pub(crate) message: CanonicalMessage,
    pub(crate) admitted_at: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ContextCursor {
    pub(crate) after: u64,
    pub(crate) catalog: Option<PromptCatalog>,
    pub(crate) segment: Option<FrozenSegment>,
    pub(crate) admitted: Vec<AdmittedMessage>,
    pub(crate) configuration: u64,
    pub(crate) handoff: Option<StoredEvent>,
    pub(crate) input_tokens: Option<u32>,
    pub(crate) events_read: u64,
    latest_turn: Option<u64>,
    tail: Vec<StoredEvent>,
    pending_admissions: Vec<StoredEvent>,
    pending_source_head: Option<u64>,
    retained_bytes: usize,
    approved_segment: u64,
}

impl ContextCursor {
    fn read_assets(&mut self, path: &Path, agent: &str) -> Result<()> {
        let mut candidate = None;
        self.after = scan(path, self.after, |event| {
            if let EventPayload::Unknown(value) = event.payload {
                if value["agent"] == agent {
                    if value["kind"] == "runtime.context_segment" {
                        candidate = Some((event.id, value.get("catalog").cloned()));
                    } else if value["kind"] == "runtime.step_prepared" {
                        if let Some((_, catalog)) =
                            candidate.as_ref().filter(|(id, _)| value["segment"] == *id)
                        {
                            self.catalog = catalog
                                .as_ref()
                                .filter(|value| !value.is_null())
                                .map(|value| serde_json::from_value(value.clone()))
                                .transpose()?;
                        }
                    }
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub(crate) fn refresh(&mut self, path: &Path, agent: &str, max_bytes: usize) -> Result<()> {
        if self.after == 0 {
            let mut latest = None;
            let mut candidate = None;
            let mut latest_turn = None;
            let mut segment_turn = None;
            let mut seen = 0;
            scan(path, 0, |event| {
                seen += 1;
                if let EventPayload::TurnStarted {
                    turn, conversation, ..
                } = &event.payload
                {
                    if conversation == agent {
                        latest_turn = Some(*turn);
                    }
                }
                if let EventPayload::Unknown(value) = &event.payload {
                    if value["kind"] == "runtime.operation"
                        && value["record"]["address"] == agent
                        && matches!(
                            value["record"]["operation"].as_str(),
                            Some("agent_created" | "agent_configured")
                        )
                    {
                        self.configuration = event.id;
                    }
                    if value["agent"] == agent {
                        if value["kind"] == "runtime.context_segment" {
                            candidate = Some((event.clone(), latest_turn));
                        }
                        if value["kind"] == "runtime.step_prepared" {
                            if let Some((segment, turn)) = candidate
                                .as_ref()
                                .filter(|(segment, _)| value["segment"] == segment.id)
                            {
                                segment_turn = *turn;
                                latest = Some(segment.clone());
                            }
                        }
                    }
                }
                Ok(())
            })?;
            self.events_read += seen;
            if let Some(event) = latest {
                self.after = event.id;
                self.approved_segment = event.id;
                self.latest_turn = segment_turn;
                self.observe(event, agent, max_bytes)?;
            }
        }
        let mut seen = 0;
        self.after = scan(path, self.after, |event| {
            seen += 1;
            self.observe(event, agent, max_bytes)
        })?;
        self.events_read += seen;
        Ok(())
    }

    pub(crate) fn observe(
        &mut self,
        event: StoredEvent,
        agent: &str,
        max_bytes: usize,
    ) -> Result<()> {
        if let EventPayload::Unknown(value) = &event.payload {
            let record = &value["record"];
            if value["kind"] == "runtime.operation"
                && record["operation"] == "message_rewind"
                && record["address"] == agent
            {
                let target_event_id = record["target_event_id"].as_u64().ok_or_else(|| {
                    Error::InvalidEventStream("message rewind has no target event".into())
                })?;
                self.admitted.retain(|message| {
                    message.admitted_at < target_event_id || message.admitted_at >= event.id
                });
                self.pending_admissions
                    .retain(|admission| admission.id < target_event_id || admission.id >= event.id);
                self.tail
                    .retain(|source| source.id < target_event_id || source.id >= event.id);
                if self.pending_admissions.is_empty() {
                    self.pending_source_head = None;
                }
                self.recount()?;
                return Ok(());
            }
        }
        if let EventPayload::TurnStarted {
            turn, conversation, ..
        } = &event.payload
        {
            if conversation == agent {
                self.latest_turn = Some(*turn);
            }
        }
        if let EventPayload::Unknown(value) = &event.payload {
            if value["kind"] == "runtime.operation"
                && value["record"]["address"] == agent
                && matches!(
                    value["record"]["operation"].as_str(),
                    Some("agent_created" | "agent_configured")
                )
            {
                self.configuration = event.id;
            }
            if value["agent"] == agent {
                match value["kind"].as_str() {
                    Some("runtime.context_segment") => {
                        if event.id != self.approved_segment {
                            return Ok(());
                        }
                        if self
                            .segment
                            .as_ref()
                            .is_none_or(|segment| segment.id != event.id)
                        {
                            let mut segment: FrozenSegment = serde_json::from_value(value.clone())?;
                            segment.id = event.id;
                            self.catalog = Some(segment.catalog.clone());
                            self.segment = Some(segment);
                            self.admitted.clear();
                            self.tail.clear();
                            self.pending_admissions.clear();
                            self.pending_source_head = None;
                            self.retained_bytes = 0;
                        }
                        return Ok(());
                    }
                    Some("runtime.context_admitted")
                        if self
                            .segment
                            .as_ref()
                            .is_some_and(|segment| value["segment"] == segment.id) =>
                    {
                        let ordinal = value["ordinal"].as_u64().ok_or_else(|| {
                            Error::InvalidEventStream("context admission has no ordinal".into())
                        })? as usize;
                        if ordinal < self.admitted.len() {
                            let message: CanonicalMessage =
                                serde_json::from_value(value["message"].clone())?;
                            if self.admitted[ordinal].message != message {
                                return Err(Error::InvalidEventStream(
                                    "committed context admission changed".into(),
                                ));
                            }
                            return Ok(());
                        }
                        let source_head = value["source_head"].as_u64();
                        if source_head != self.pending_source_head
                            || self.pending_admissions.first().is_some_and(|event| {
                                matches!(&event.payload, EventPayload::Unknown(value)
                                    if value["ordinal"].as_u64().is_some_and(|first| ordinal as u64 <= first))
                            })
                        {
                            self.pending_admissions.clear();
                            self.pending_source_head = source_head;
                            self.recount()?;
                        }
                        self.reserve(&event, max_bytes)?;
                        self.pending_admissions.push(event);
                        return Ok(());
                    }
                    Some("runtime.step_prepared") if value["segment"] == self.approved_segment => {
                        self.confirm_admissions(value)?;
                        let source_head = value["source_head"].as_u64().unwrap_or(event.id);
                        self.tail.retain(|event| event.id > source_head);
                        self.recount()?;
                        return Ok(());
                    }
                    Some(_) | None => {}
                }
            }
        }
        if matches!(&event.payload, EventPayload::Handoff { turn, .. } if Some(*turn) == self.latest_turn)
        {
            self.handoff = Some(event);
            return Ok(());
        }
        if let EventPayload::ModelResponse {
            conversation,
            turn,
            input_tokens_total: Some(tokens),
            ..
        } = &event.payload
        {
            if belongs(conversation.as_deref(), *turn, agent, self.latest_turn) {
                self.input_tokens = Some((*tokens).min(u64::from(u32::MAX)) as u32);
            }
        }
        if let Some(event) = project_source(event, agent, self.latest_turn) {
            self.reserve(&event, max_bytes)?;
            self.tail.push(event);
        }
        Ok(())
    }

    pub(crate) fn history(&self, conversation: &ConversationAddress) -> Vec<AdmittedMessage> {
        let mut result = self.admitted.clone();
        let mut turns = BTreeMap::<u64, Vec<StoredEvent>>::new();
        for event in &self.tail {
            if let Some(turn) = source_turn(&event.payload) {
                turns.entry(turn).or_default().push(event.clone());
            }
        }
        for (turn, mut events) in turns {
            let admitted_at = events
                .iter()
                .map(|event| event.id)
                .max()
                .unwrap_or_default();
            events.insert(
                0,
                StoredEvent {
                    id: 0,
                    payload: EventPayload::TurnStarted {
                        turn,
                        ts: String::new(),
                        conversation: conversation.as_str().into(),
                    },
                },
            );
            let (_, messages) = super::replay::rebuild_history_for_provider(&events, conversation);
            result.extend(messages.into_iter().map(|message| AdmittedMessage {
                turn,
                origin: MessageOrigin::of(&message),
                message,
                admitted_at,
            }));
        }
        result
    }

    fn confirm_admissions(&mut self, prepared: &serde_json::Value) -> Result<()> {
        let head = prepared["context_head"].as_u64().ok_or_else(|| {
            Error::InvalidEventStream("prepared context has no admission boundary".into())
        })? as usize;
        if head < self.admitted.len() {
            return Err(Error::InvalidEventStream(
                "prepared context removed its committed admission prefix".into(),
            ));
        }
        let count = head - self.admitted.len();
        if count > 0 {
            if self.pending_source_head.is_some()
                && prepared["source_head"].as_u64().is_some()
                && self.pending_source_head != prepared["source_head"].as_u64()
            {
                return Err(Error::InvalidEventStream(
                    "prepared context does not reference its pending admission batch".into(),
                ));
            }
            if self.pending_admissions.len() != count {
                return Err(Error::InvalidEventStream(
                    "prepared context does not match its admission batch length".into(),
                ));
            }
        }
        for event in std::mem::take(&mut self.pending_admissions)
            .into_iter()
            .take(count)
        {
            let admitted_at = event.id;
            let EventPayload::Unknown(value) = event.payload else {
                unreachable!("pending context admissions are runtime facts")
            };
            if value["ordinal"].as_u64() != Some(self.admitted.len() as u64) {
                return Err(Error::InvalidEventStream(
                    "context admissions are not contiguous".into(),
                ));
            }
            let message: CanonicalMessage = serde_json::from_value(value["message"].clone())?;
            let origin = value
                .get("origin")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_else(|| MessageOrigin::of(&message));
            self.admitted.push(AdmittedMessage {
                turn: value["turn"].as_u64().or(self.latest_turn).unwrap_or(0),
                origin,
                message,
                admitted_at: value["admitted_at"].as_u64().unwrap_or(admitted_at),
            });
        }
        self.pending_source_head = None;
        if self.admitted.len() != head {
            return Err(Error::InvalidEventStream(
                "prepared context admission batch is incomplete".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn commit(
        &mut self,
        segment: FrozenSegment,
        admitted: Vec<AdmittedMessage>,
        source_head: u64,
    ) -> Result<()> {
        self.approved_segment = segment.id;
        self.catalog = Some(segment.catalog.clone());
        self.segment = Some(segment);
        self.admitted = admitted;
        self.pending_admissions.clear();
        self.pending_source_head = None;
        self.tail.retain(|event| event.id > source_head);
        self.recount()
    }

    fn reserve(&mut self, value: &impl serde::Serialize, max_bytes: usize) -> Result<()> {
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(serde_json::to_vec(value)?.len());
        if self.retained_bytes > max_bytes.saturating_mul(MAX_SELECTION_BYTES_FACTOR) {
            return Err(Error::InvalidArgument(
                "active context selection exceeds its request-byte budget".into(),
            ));
        }
        Ok(())
    }

    fn recount(&mut self) -> Result<()> {
        self.retained_bytes = 0;
        for admitted in &self.admitted {
            self.retained_bytes += serde_json::to_vec(&admitted.message)?.len();
        }
        for event in &self.tail {
            self.retained_bytes += serde_json::to_vec(event)?.len();
        }
        for event in &self.pending_admissions {
            self.retained_bytes += serde_json::to_vec(event)?.len();
        }
        Ok(())
    }
}

fn scan(
    path: &Path,
    mut after: u64,
    mut visit: impl FnMut(StoredEvent) -> Result<()>,
) -> Result<u64> {
    loop {
        let page = EventStore::read_page(path, after, PAGE_RECORDS)?;
        if page.is_empty() {
            return Ok(after);
        }
        for event in page {
            after = event.id;
            visit(event)?;
        }
    }
}

fn belongs(conversation: Option<&str>, turn: u64, agent: &str, latest_turn: Option<u64>) -> bool {
    conversation.map_or(Some(turn) == latest_turn, |conversation| {
        conversation == agent
    })
}

fn source_turn(payload: &EventPayload) -> Option<u64> {
    match payload {
        EventPayload::MessageUser { turn, .. }
        | EventPayload::MessageAssistant { turn, .. }
        | EventPayload::ModelResponse { turn, .. }
        | EventPayload::ToolCall { turn, .. }
        | EventPayload::ToolResult { turn, .. } => Some(*turn),
        EventPayload::Unknown(value) if value["kind"] == "runtime.provider_replay" => {
            value["turn"].as_u64()
        }
        _ => None,
    }
}

fn project_source(
    mut event: StoredEvent,
    agent: &str,
    latest_turn: Option<u64>,
) -> Option<StoredEvent> {
    let selected = match &event.payload {
        EventPayload::MessageUser { conversation, .. }
        | EventPayload::MessageAssistant { conversation, .. } => conversation == agent,
        EventPayload::ModelResponse {
            conversation, turn, ..
        }
        | EventPayload::ToolCall {
            conversation, turn, ..
        }
        | EventPayload::ToolResult {
            conversation, turn, ..
        } => belongs(conversation.as_deref(), *turn, agent, latest_turn),
        EventPayload::Unknown(value) => {
            value["kind"] == "runtime.provider_replay" && value["agent"] == agent
        }
        _ => false,
    };
    if !selected {
        return None;
    }
    match &mut event.payload {
        EventPayload::ModelResponse { conversation, .. }
        | EventPayload::ToolCall { conversation, .. }
        | EventPayload::ToolResult { conversation, .. }
            if conversation.is_none() =>
        {
            *conversation = Some(agent.into())
        }
        _ => {}
    }
    if let EventPayload::ToolResult {
        structured,
        files_read,
        files_changed,
        commands_run,
        memory_changed,
        ..
    } = &mut event.payload
    {
        *structured = None;
        files_read.clear();
        files_changed.clear();
        commands_run.clear();
        *memory_changed = None;
    }
    Some(event)
}

pub(crate) fn read_frozen_catalog(path: &Path, agent: &str) -> Result<Option<PromptCatalog>> {
    let mut cursor = ContextCursor::default();
    cursor.read_assets(path, agent)?;
    Ok(cursor.catalog)
}

pub(crate) fn read_agent_events(
    path: &Path,
    agent: &str,
    max_bytes: usize,
) -> Result<Vec<StoredEvent>> {
    let mut latest_turn = None;
    let mut metadata = BTreeMap::<String, StoredEvent>::new();
    let mut current = Vec::<StoredEvent>::new();
    let mut skills = Vec::<StoredEvent>::new();
    let mut reads = BTreeMap::<String, (u64, StoredEvent)>::new();
    scan(path, 0, |mut event| {
        let mut changed = false;
        let key = match &event.payload {
            EventPayload::SessionCreated { .. } => Some("session".into()),
            EventPayload::ConversationOpened { conversation, .. } if conversation == agent => {
                Some("opened".into())
            }
            EventPayload::ConversationBound { conversation, .. } if conversation == agent => {
                Some("bound".into())
            }
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
            metadata.insert(key, event);
            return Ok(());
        }
        if let EventPayload::ContextSkills { conversation, .. } = &event.payload {
            if conversation == agent {
                if skills.len() == 2 {
                    skills.remove(0);
                }
                skills.push(event);
            }
            return Ok(());
        }
        if let EventPayload::TurnStarted {
            turn, conversation, ..
        } = &event.payload
        {
            if conversation == agent {
                latest_turn = Some(*turn);
                current.clear();
                current.push(event);
            }
            return Ok(());
        }
        if let EventPayload::ToolResult {
            conversation,
            turn,
            status,
            structured: Some(value),
            ..
        } = &event.payload
        {
            if belongs(conversation.as_deref(), *turn, agent, latest_turn)
                && status == "ok"
                && value["kind"] == "file_content"
            {
                let key = format!(
                    "{}:{}",
                    value["canonical_path"].as_str().unwrap_or(""),
                    value["start_line"].as_u64().unwrap_or(1)
                );
                if value["cached"] != true {
                    reads.insert(key, (event.id, event.clone()));
                } else if let Some((accessed, _)) = reads.get_mut(&key) {
                    *accessed = event.id;
                }
                changed = true;
                if reads.len() > MAX_CACHED_READS {
                    if let Some(key) = reads
                        .iter()
                        .min_by_key(|(_, (accessed, _))| *accessed)
                        .map(|(key, _)| key.clone())
                    {
                        reads.remove(&key);
                    }
                }
            }
        }
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
            } if belongs(conversation.as_deref(), *turn, agent, latest_turn) => Some(*turn),
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
                        Some("runtime.tool_prepared" | "runtime.tool_dispatched")
                    ) =>
            {
                value["turn"].as_u64()
            }
            _ => None,
        };
        if turn.is_some() && turn == latest_turn {
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
                *structured = None;
                files_read.clear();
                files_changed.clear();
                commands_run.clear();
                *memory_changed = None;
            }
            current.push(event);
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        trim_read_cache(&metadata, &current, &skills, &mut reads, max_bytes)?;
        Ok(())
    })?;
    trim_read_cache(&metadata, &current, &skills, &mut reads, max_bytes)?;
    let selected = metadata
        .into_values()
        .chain(current)
        .chain(skills)
        .chain(reads.into_values().map(|(_, event)| event))
        .map(|event| (event.id, event))
        .collect::<BTreeMap<_, _>>();
    let result = selected.into_values().collect::<Vec<_>>();
    if serde_json::to_vec(&result)?.len() > max_bytes.saturating_mul(MAX_SELECTION_BYTES_FACTOR) {
        return Err(Error::InvalidArgument(
            "agent startup facts exceed the context selection byte limit".into(),
        ));
    }
    Ok(result)
}

fn trim_read_cache(
    metadata: &BTreeMap<String, StoredEvent>,
    current: &[StoredEvent],
    skills: &[StoredEvent],
    reads: &mut BTreeMap<String, (u64, StoredEvent)>,
    max_bytes: usize,
) -> Result<()> {
    let budget = max_bytes.saturating_mul(MAX_SELECTION_BYTES_FACTOR);
    while serde_json::to_vec(&(metadata, current, skills, &reads))?.len() > budget {
        let Some(key) = reads
            .iter()
            .min_by_key(|(_, (accessed, _))| *accessed)
            .map(|(key, _)| key.clone())
        else {
            return Err(Error::InvalidArgument(
                "agent startup facts exceed the context selection byte limit".into(),
            ));
        };
        reads.remove(&key);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
