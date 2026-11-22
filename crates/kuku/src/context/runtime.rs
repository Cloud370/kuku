use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::context::{CanonicalMessage, MessageBlock, Role, ToolSchema};
use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::prompt::PromptCatalog;
use crate::provider::types::ProviderCapabilities;
use crate::{Error, Result};

mod startup;
pub(crate) use startup::read_agent_events;

const PAGE_RECORDS: usize = 256;
const MAX_SELECTION_BYTES_FACTOR: usize = 4;
const MAX_CACHED_FILE_SNAPSHOTS: usize = 128;

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
    pub(crate) capabilities: Option<ProviderCapabilities>,
    #[serde(default)]
    pub(crate) route_selectors: Option<RouteSelectors>,
    #[serde(default)]
    pub(crate) behavior: String,
    #[serde(default = "identity_slot")]
    pub(crate) identity_slot: usize,
    #[serde(default)]
    pub(crate) handoff_prelude: Option<usize>,
    #[serde(default)]
    pub(crate) workspace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RouteSelectors {
    pub(crate) query_connection: Option<String>,
    pub(crate) query_model: Option<String>,
    pub(crate) query_tier: Option<String>,
}

fn route_selectors_changed(segment: Option<&FrozenSegment>, config: &serde_json::Value) -> bool {
    let Some(recorded) = segment.and_then(|segment| segment.route_selectors.as_ref()) else {
        return false;
    };
    let selectors = RouteSelectors {
        query_connection: config["connection"].as_str().map(str::to_owned),
        query_model: config["model"].as_str().map(str::to_owned),
        query_tier: config["tier"].as_str().map(str::to_owned),
    };
    recorded != &selectors
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
    Checkpoint,
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
    pub(crate) configured: u64,
    configurations: BTreeMap<u64, RouteSelectors>,
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
    fn observe_configuration(&mut self, event_id: u64, config: &serde_json::Value) {
        self.configuration = event_id;
        self.configurations.insert(
            event_id,
            RouteSelectors {
                query_connection: config["connection"].as_str().map(str::to_owned),
                query_model: config["model"].as_str().map(str::to_owned),
                query_tier: config["tier"].as_str().map(str::to_owned),
            },
        );
    }

    pub(crate) fn select_configuration(&mut self, selectors: &RouteSelectors) {
        let matching = self
            .configurations
            .iter()
            .rev()
            .find(|(_, recorded)| *recorded == selectors)
            .map(|(event_id, _)| *event_id);
        if let Some(event_id) = matching {
            self.configuration = event_id;
            if self.segment.as_ref().is_some_and(|segment| {
                segment
                    .route_selectors
                    .as_ref()
                    .is_some_and(|recorded| recorded != selectors)
            }) {
                self.configured = self.configured.max(event_id);
            }
        }
    }

    fn read_assets(&mut self, path: &Path, agent: &str) -> Result<()> {
        if let Some(segment) = committed_segments(path, agent, |_| {})?.0.last() {
            let event = read_segment(path, segment.id)?;
            if let EventPayload::Unknown(value) = event.payload {
                self.catalog = value
                    .get("catalog")
                    .filter(|catalog| !catalog.is_null())
                    .map(|catalog| serde_json::from_value(catalog.clone()))
                    .transpose()?;
            }
        }
        Ok(())
    }

    pub(crate) fn refresh(&mut self, path: &Path, agent: &str, max_bytes: usize) -> Result<()> {
        if self.after == 0 {
            let (committed, seen) = committed_segments(path, agent, |event| {
                if let EventPayload::Unknown(value) = &event.payload {
                    let record = &value["record"];
                    if value["kind"] != "runtime.operation" {
                        return;
                    }
                    match record["operation"].as_str() {
                        Some("agent_created" | "agent_configured")
                            if record["address"] == agent =>
                        {
                            self.observe_configuration(event.id, &record["config"]);
                        }
                        Some("input_delivered")
                            if record["input"]["target"] == agent
                                && record["config"].is_object() =>
                        {
                            self.observe_configuration(event.id, &record["config"]);
                        }
                        _ => {}
                    }
                }
            })?;
            self.events_read += seen;
            if let Some(segment) = committed.last() {
                self.after = segment.id;
                self.approved_segment = segment.id;
                self.latest_turn = segment.turn;
                self.observe(read_segment(path, segment.id)?, agent, max_bytes)?;
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
                let target_event_id = rewind_target(record)?;
                self.admitted.retain(|message| {
                    message.admitted_at < target_event_id || message.admitted_at >= event.id
                });
                self.pending_admissions
                    .retain(|admission| admission.id < target_event_id || admission.id >= event.id);
                self.tail
                    .retain(|source| source.id < target_event_id || source.id >= event.id);
                if self
                    .handoff
                    .as_ref()
                    .is_some_and(|handoff| handoff.id >= target_event_id)
                {
                    self.handoff = None;
                }
                if self
                    .segment
                    .as_ref()
                    .is_some_and(|segment| segment.id >= target_event_id)
                {
                    self.segment = None;
                }
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
            if value["kind"] == "runtime.operation" {
                let record = &value["record"];
                match record["operation"].as_str() {
                    Some("agent_created") if record["address"] == agent => {
                        self.observe_configuration(event.id, &record["config"]);
                    }
                    Some("agent_configured") if record["address"] == agent => {
                        self.observe_configuration(event.id, &record["config"]);
                        if route_selectors_changed(self.segment.as_ref(), &record["config"]) {
                            self.configured = event.id;
                        }
                    }
                    Some("input_delivered")
                        if record["input"]["target"] == agent && record["config"].is_object() =>
                    {
                        self.observe_configuration(event.id, &record["config"]);
                        if route_selectors_changed(self.segment.as_ref(), &record["config"]) {
                            self.configured = event.id;
                        }
                    }
                    _ => {}
                }
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
                    _ if context_confirmation(value)
                        && value["segment"] == self.approved_segment =>
                    {
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
            let (_, messages) =
                super::replay::rebuild_history_for_provider_with_provenance(&events, conversation);
            result.extend(
                messages
                    .into_iter()
                    .map(|(message, admitted_at)| AdmittedMessage {
                        turn,
                        origin: MessageOrigin::of(&message),
                        message,
                        admitted_at,
                    }),
            );
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

struct SegmentCommit {
    id: u64,
    turn: Option<u64>,
    checkpoint: bool,
}

pub(crate) fn latest_checkpoint_segment(path: &Path, agent: &str) -> Result<Option<u64>> {
    let (segments, _) = committed_segments(path, agent, |_| {})?;
    Ok(segments
        .iter()
        .rev()
        .find(|segment| segment.checkpoint)
        .map(|segment| segment.id))
}

fn read_segment(path: &Path, id: u64) -> Result<StoredEvent> {
    EventStore::read_page(path, id - 1, 1)?
        .pop()
        .filter(|event| event.id == id)
        .ok_or_else(|| Error::InvalidEventStream("committed context segment is missing".into()))
}

fn committed_segments(
    path: &Path,
    agent: &str,
    mut visit: impl FnMut(&StoredEvent),
) -> Result<(Vec<SegmentCommit>, u64)> {
    let mut committed: Vec<SegmentCommit> = Vec::new();
    let mut candidate = None;
    let mut latest_turn = None;
    let mut seen = 0;
    scan(path, 0, |event| {
        seen += 1;
        visit(&event);
        match &event.payload {
            EventPayload::TurnStarted {
                turn, conversation, ..
            } if conversation == agent => {
                latest_turn = Some(*turn);
            }
            EventPayload::Unknown(value) => {
                let record = &value["record"];
                if value["kind"] == "runtime.operation" && record["address"] == agent {
                    let target = match record["operation"].as_str() {
                        Some("message_rewind") => Some(rewind_target(record)?),
                        Some("compaction_reverted") => {
                            Some(record["segment"].as_u64().ok_or_else(|| {
                                Error::InvalidEventStream("compaction rewind has no segment".into())
                            })?)
                        }
                        _ => None,
                    };
                    if let Some(target) = target {
                        committed.retain(|entry| entry.id < target);
                        if candidate.is_some_and(|(id, _)| id >= target) {
                            candidate = None;
                        }
                    }
                }
                if value["agent"] == agent {
                    if value["kind"] == "runtime.context_segment" {
                        candidate = Some((event.id, latest_turn));
                    }
                    if context_confirmation(value) {
                        if let Some((id, turn)) =
                            candidate.filter(|(id, _)| value["segment"] == *id)
                        {
                            committed.push(SegmentCommit {
                                id,
                                turn,
                                checkpoint: value["kind"] == "runtime.compaction_finished",
                            });
                            candidate = None;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok((committed, seen))
}

fn context_confirmation(value: &serde_json::Value) -> bool {
    value["kind"] == "runtime.step_prepared"
        || (value["kind"] == "runtime.compaction_finished" && value["status"] == "completed")
}

fn rewind_target(record: &serde_json::Value) -> Result<u64> {
    record["target_event_id"]
        .as_u64()
        .ok_or_else(|| Error::InvalidEventStream("message rewind has no target event".into()))
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
        *structured = structured
            .as_ref()
            .and_then(crate::execution::ModelImage::from_tool_result)
            .map(|image| image.model_facts());
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SnapshotKey {
    Read(String, u64),
    Write(u64, String),
}

#[derive(Default)]
struct SnapshotCache {
    entries: BTreeMap<SnapshotKey, FileSnapshot>,
    visible_writes: BTreeSet<u64>,
    segment: Option<u64>,
    pending: Option<WriteAdmissions>,
}

struct FileSnapshot {
    accessed: u64,
    event: StoredEvent,
    event_bytes: usize,
}

struct WriteAdmissions {
    segment: u64,
    source_head: u64,
    next_ordinal: u64,
    receipts: BTreeSet<u64>,
}

impl SnapshotCache {
    fn observe_context(&mut self, value: &serde_json::Value) {
        let (Some(segment), Some(source_head)) =
            (value["segment"].as_u64(), value["source_head"].as_u64())
        else {
            return;
        };
        match value["kind"].as_str() {
            Some("runtime.context_admitted") => self.admit(value, segment, source_head),
            Some("runtime.step_prepared") => {
                if self.segment != Some(segment) {
                    self.visible_writes.clear();
                    self.segment = Some(segment);
                }
                if let Some(batch) = self.pending.take().filter(|batch| {
                    batch.segment == segment
                        && batch.source_head == source_head
                        && Some(batch.next_ordinal) == value["context_head"].as_u64()
                }) {
                    self.visible_writes.extend(batch.receipts);
                }
                self.entries.retain(|key, snapshot| {
                    matches!(key, SnapshotKey::Read(..))
                        || snapshot.event.id > source_head
                        || self.visible_writes.contains(&snapshot.event.id)
                });
            }
            _ => {}
        }
    }

    fn admit(&mut self, value: &serde_json::Value, segment: u64, source_head: u64) {
        let (Some(ordinal), Some(turn), Some(blocks)) = (
            value["ordinal"].as_u64(),
            value["turn"].as_u64(),
            value["message"]["blocks"].as_array(),
        ) else {
            return;
        };
        if self.pending.as_ref().is_none_or(|batch| {
            batch.segment != segment
                || batch.source_head != source_head
                || batch.next_ordinal != ordinal
        }) {
            self.pending = Some(WriteAdmissions {
                segment,
                source_head,
                next_ordinal: ordinal,
                receipts: BTreeSet::new(),
            });
        }
        let batch = self.pending.as_mut().expect("matching admission batch");
        batch.next_ordinal = ordinal.saturating_add(1);
        for block in blocks {
            let result = &block["ToolResult"];
            let Some(call) = result["tool_call_id"]
                .as_str()
                .filter(|_| result["status"] == "ok")
            else {
                continue;
            };
            if let Some(snapshot) = self
                .entries
                .get(&SnapshotKey::Write(turn, call.into()))
                .filter(|snapshot| snapshot.event.id <= source_head)
            {
                batch.receipts.insert(snapshot.event.id);
            }
        }
    }

    fn observe_result(
        &mut self,
        event: &mut StoredEvent,
        agent: &str,
        latest_turn: Option<u64>,
    ) -> Result<bool> {
        let EventPayload::ToolResult {
            conversation,
            turn,
            tool_call_id,
            status,
            summary,
            model_content,
            structured: Some(value),
            ..
        } = &mut event.payload
        else {
            return Ok(false);
        };
        if status != "ok" || !belongs(conversation.as_deref(), *turn, agent, latest_turn) {
            return Ok(false);
        }
        let key = if value["kind"] == "file_content" {
            let Some(path) = value["canonical_path"].as_str() else {
                return Ok(false);
            };
            let key = SnapshotKey::Read(path.into(), value["start_line"].as_u64().unwrap_or(1));
            if value["cached"] == true {
                if let Some(snapshot) = self.entries.get_mut(&key) {
                    snapshot.accessed = event.id;
                }
                return Ok(true);
            }
            key
        } else if let Some((path, hash)) = super::replay::supplied_file_version(value) {
            *value = serde_json::json!({
                "kind": "file_write", "canonical_path": path, "content_hash_after": hash,
                "commit_state": "committed", (super::replay::SUPPLIED_FULL_WRITE): true,
            });
            summary.clear();
            model_content.clear();
            SnapshotKey::Write(*turn, tool_call_id.clone())
        } else {
            return Ok(false);
        };
        *conversation = Some(agent.into());
        self.entries.insert(
            key,
            FileSnapshot {
                accessed: event.id,
                event_bytes: serde_json::to_vec(event)?.len(),
                event: event.clone(),
            },
        );
        if self.entries.len() > MAX_CACHED_FILE_SNAPSHOTS {
            self.remove_oldest();
        }
        Ok(true)
    }

    fn remove_oldest(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, snapshot)| snapshot.accessed)
            .map(|(key, _)| key.clone())
        else {
            return false;
        };
        let snapshot = self.entries.remove(&key).expect("oldest snapshot exists");
        self.visible_writes.remove(&snapshot.event.id);
        if let Some(batch) = self.pending.as_mut() {
            batch.receipts.remove(&snapshot.event.id);
        }
        true
    }

    fn into_events(self) -> impl Iterator<Item = StoredEvent> {
        self.entries.into_iter().filter_map(move |(key, snapshot)| {
            (matches!(key, SnapshotKey::Read(..))
                || self.visible_writes.contains(&snapshot.event.id))
            .then_some(snapshot.event)
        })
    }
}

#[cfg(test)]
mod tests;
