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
    retained_bytes: usize,
    approved_segment: u64,
    admission_limit: usize,
}

impl ContextCursor {
    fn read_assets(&mut self, path: &Path, agent: &str) -> Result<()> {
        self.after = scan(path, self.after, |event| {
            if let EventPayload::Unknown(value) = event.payload {
                if value["kind"] == "runtime.context_segment" && value["agent"] == agent {
                    self.catalog = value
                        .get("catalog")
                        .filter(|value| !value.is_null())
                        .map(|value| serde_json::from_value(value.clone()))
                        .transpose()?;
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
                                self.admission_limit =
                                    value["context_head"].as_u64().unwrap_or(0) as usize;
                                segment_turn = *turn;
                                latest = Some(segment.clone());
                            } else if latest
                                .as_ref()
                                .is_some_and(|segment: &StoredEvent| value["segment"] == segment.id)
                            {
                                self.admission_limit =
                                    value["context_head"].as_u64().unwrap_or(0) as usize;
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
                        if ordinal >= self.admission_limit {
                            return Ok(());
                        }
                        let message: CanonicalMessage =
                            serde_json::from_value(value["message"].clone())?;
                        if ordinal < self.admitted.len() {
                            if self.admitted[ordinal].message != message {
                                return Err(Error::InvalidEventStream(
                                    "committed context admission changed".into(),
                                ));
                            }
                        } else {
                            if ordinal != self.admitted.len() {
                                return Err(Error::InvalidEventStream(
                                    "context admissions are not contiguous".into(),
                                ));
                            }
                            self.reserve(&message, max_bytes)?;
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
                            });
                        }
                        return Ok(());
                    }
                    Some("runtime.step_prepared") if value["segment"] == self.approved_segment => {
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
            let (_, messages) = super::replay::rebuild_history_for_provider(&events, conversation);
            result.extend(messages.into_iter().map(|message| AdmittedMessage {
                turn,
                origin: MessageOrigin::of(&message),
                message,
            }));
        }
        result
    }

    pub(crate) fn commit(
        &mut self,
        segment: FrozenSegment,
        admitted: Vec<AdmittedMessage>,
        source_head: u64,
    ) -> Result<()> {
        self.approved_segment = segment.id;
        self.admission_limit = admitted.len();
        self.catalog = Some(segment.catalog.clone());
        self.segment = Some(segment);
        self.admitted = admitted;
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_result_projection_keeps_the_wire_view_in_every_protocol() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let render = |events: &[StoredEvent]| {
            let (_, history) = super::super::replay::rebuild_history_for_provider(
                events,
                &ConversationAddress::MAIN,
            );
            let request = crate::provider::types::ProviderRequest {
                assembly: crate::context::ContextAssembly {
                    system_prompt: String::new(),
                    prelude_messages: Vec::new(),
                    history,
                    tools: Vec::new(),
                    prompt_asset_sources: Vec::new(),
                    project_instruction_sources: Vec::new(),
                    memory_sources: Vec::new(),
                    runtime_context: None,
                    handoff_summary: None,
                },
                catalog: &catalog,
                current_input: crate::provider::types::CanonicalPromptInput { parts: Vec::new() },
                model: "fixture".into(),
                max_output_tokens: Some(128),
                temperature: None,
                stream: true,
                think_level: crate::config::ThinkLevel::Off,
                thinking: Default::default(),
            };
            [
                crate::provider::openai_compat::render_body(&request),
                crate::provider::anthropic::render_body(&request),
                crate::provider::openai_responses::render_body(&request),
            ]
        };
        for status in ["ok", "error", "cancelled"] {
            for model_content in ["", "EXACT_MODEL_VIEW"] {
                let events = vec![
                    StoredEvent {
                        id: 1,
                        payload: EventPayload::ToolCall {
                            turn: 1,
                            ts: String::new(),
                            conversation: Some("main".into()),
                            tool_call_id: "tool".into(),
                            request_id: "req_1".into(),
                            index: 0,
                            tool: "fixture".into(),
                            args: json!({}),
                        },
                    },
                    StoredEvent {
                        id: 2,
                        payload: EventPayload::ToolResult {
                            turn: 1,
                            ts: String::new(),
                            conversation: Some("main".into()),
                            tool_call_id: "tool".into(),
                            status: status.into(),
                            summary: "DIAGNOSTIC_SUMMARY".into(),
                            model_content: model_content.into(),
                            truncated: true,
                            files_read: vec!["read".into()],
                            files_changed: vec!["changed".into()],
                            commands_run: vec!["command".into()],
                            memory_changed: Some(json!({"diagnostic":true})),
                            structured: Some(json!({"diagnostic":"x".repeat(65_536)})),
                        },
                    },
                ];
                let projected = events
                    .iter()
                    .cloned()
                    .filter_map(|event| project_source(event, "main", Some(1)))
                    .collect::<Vec<_>>();
                assert!(matches!(
                    &projected[1].payload,
                    EventPayload::ToolResult {
                        structured: None,
                        ..
                    }
                ));
                let original = render(&events);
                assert_eq!(original, render(&projected));
                assert_eq!(
                    model_content,
                    original[0]["messages"].as_array().unwrap().last().unwrap()["content"]
                );
                let messages = original[1]["messages"].as_array().unwrap();
                assert_eq!(
                    model_content,
                    messages.last().unwrap()["content"][0]["content"]
                );
                assert_eq!(
                    status != "ok",
                    messages.last().unwrap()["content"][0]["is_error"]
                );
                assert_eq!(
                    model_content,
                    original[2]["input"].as_array().unwrap().last().unwrap()["output"]
                );
            }
        }
    }

    #[test]
    fn frozen_assets_are_selected_by_agent_across_bounded_pages() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut writer = EventStore::open(&path).unwrap();
        let mut original = crate::prompt::builtin_prompt_catalog();
        original.system.text = "original main system".into();
        writer
            .append(EventPayload::Unknown(
                json!({"kind":"runtime.context_segment","agent":"main","catalog":original}),
            ))
            .unwrap();
        for index in 0..300 {
            writer.append(EventPayload::Unknown(json!({"kind":"fixture.other_agent_output","agent":"reviewer","index":index,"output":"x".repeat(4096)}))).unwrap();
        }
        let mut unrelated = original.clone();
        unrelated.system.text = "other agent system".into();
        writer
            .append(EventPayload::Unknown(
                json!({"kind":"runtime.context_segment","agent":"reviewer","catalog":unrelated}),
            ))
            .unwrap();
        assert_eq!(Some(original), read_frozen_catalog(&path, "main").unwrap());
        assert_eq!(None, read_frozen_catalog(&path, "missing").unwrap());
    }

    fn segment(agent: &str, system: &str) -> EventPayload {
        EventPayload::Unknown(
            json!({"kind":"runtime.context_segment","agent":agent,"signature":"fixture","configuration":0,"handoff":0,
            "system_prompt":system,"prelude":[],"tools":[],"catalog":crate::prompt::builtin_prompt_catalog()}),
        )
    }

    #[test]
    fn active_cursor_selects_one_prepared_segment_and_incrementally_reads_new_facts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut writer = EventStore::open(&path).unwrap();
        writer
            .append(EventPayload::TurnStarted {
                ts: "ts".into(),
                conversation: "reviewer".into(),
                turn: 1,
            })
            .unwrap();
        let input = writer
            .append(EventPayload::MessageUser {
                ts: "ts".into(),
                conversation: "reviewer".into(),
                turn: 1,
                text: "input".into(),
                from: None,
                via_tool_call_id: None,
            })
            .unwrap();
        let first = writer
            .append(segment("reviewer", "committed system"))
            .unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"reviewer","segment":first.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("input")}))).unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.step_prepared","agent":"reviewer","segment":first.id,"context_head":1,"source_head":input.id}))).unwrap();
        writer
            .append(EventPayload::TurnStarted {
                ts: "ts".into(),
                conversation: "noisy".into(),
                turn: 2,
            })
            .unwrap();
        for _ in 0..40 {
            writer
                .append(EventPayload::ModelResponse {
                    ts: "ts".into(),
                    conversation: Some("noisy".into()),
                    turn: 2,
                    request_id: "noise".into(),
                    text: "x".repeat(64 * 1024),
                    thinking: None,
                    stop_reason: Some(crate::event::ModelStopReason::EndTurn),
                    input_tokens_total: None,
                    output_tokens_total: None,
                })
                .unwrap();
        }
        writer
            .append(EventPayload::ModelResponse {
                ts: "ts".into(),
                conversation: Some("reviewer".into()),
                turn: 1,
                request_id: "reply".into(),
                text: "done".into(),
                thinking: None,
                stop_reason: Some(crate::event::ModelStopReason::EndTurn),
                input_tokens_total: Some(100),
                output_tokens_total: Some(1),
            })
            .unwrap();
        let orphan = writer
            .append(segment("reviewer", "unprepared replacement"))
            .unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"reviewer","segment":orphan.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("unprepared view")}))).unwrap();
        let mut cursor = ContextCursor::default();
        cursor.refresh(&path, "reviewer", 16 * 1024).unwrap();
        assert_eq!(
            Some(first.id),
            cursor.segment.as_ref().map(|segment| segment.id)
        );
        assert!(cursor.retained_bytes < 4096);
        let conversation = ConversationAddress::parse("reviewer").unwrap();
        let history = cursor.history(&conversation);
        assert_eq!(2, history.len());
        assert_eq!(
            CanonicalMessage::assistant(vec![MessageBlock::Text("done".into())]),
            history[1].message
        );
        let read = cursor.events_read;
        writer
            .append(EventPayload::MessageAssistant {
                ts: "ts".into(),
                conversation: "reviewer".into(),
                turn: 1,
                message_id: "reply".into(),
                text: "done".into(),
            })
            .unwrap();
        writer
            .append(EventPayload::MessageAssistant {
                ts: "ts".into(),
                conversation: "reviewer".into(),
                turn: 1,
                message_id: "independent".into(),
                text: "independent notice".into(),
            })
            .unwrap();
        cursor.refresh(&path, "reviewer", 16 * 1024).unwrap();
        assert_eq!(2, cursor.events_read - read);
        assert_eq!(3, cursor.history(&conversation).len());
        let read = cursor.events_read;
        cursor.refresh(&path, "reviewer", 16 * 1024).unwrap();
        assert_eq!(read, cursor.events_read);
    }

    #[test]
    fn an_orphan_segment_does_not_hide_a_later_prepared_head_of_the_active_segment() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut writer = EventStore::open(&path).unwrap();
        writer
            .append(EventPayload::TurnStarted {
                ts: "ts".into(),
                conversation: "main".into(),
                turn: 1,
            })
            .unwrap();
        let first = writer.append(segment("main", "prepared system")).unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"main","segment":first.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("first input")}))).unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.step_prepared","agent":"main","segment":first.id,"context_head":1,"source_head":first.id}))).unwrap();
        let reply = writer
            .append(EventPayload::ModelResponse {
                ts: "ts".into(),
                conversation: Some("main".into()),
                turn: 1,
                request_id: "reply".into(),
                text: "accepted response".into(),
                thinking: None,
                stop_reason: Some(crate::event::ModelStopReason::EndTurn),
                input_tokens_total: Some(100),
                output_tokens_total: Some(1),
            })
            .unwrap();
        let orphan = writer.append(segment("main", "unprepared system")).unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"main","segment":orphan.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("orphan input")}))).unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"main","segment":first.id,"ordinal":1,"turn":1,"origin":"response","message":CanonicalMessage::assistant(vec![MessageBlock::Text("accepted response".into())])}))).unwrap();
        writer.append(EventPayload::Unknown(json!({"kind":"runtime.step_prepared","agent":"main","segment":first.id,"context_head":2,"source_head":reply.id}))).unwrap();
        let mut cursor = ContextCursor::default();
        cursor.refresh(&path, "main", 16 * 1024).unwrap();
        assert_eq!(
            Some(first.id),
            cursor.segment.as_ref().map(|segment| segment.id)
        );
        assert_eq!(2, cursor.admitted.len());
        assert_eq!(
            CanonicalMessage::assistant(vec![MessageBlock::Text("accepted response".into())]),
            cursor.history(&ConversationAddress::MAIN)[1].message
        );
    }
    fn append_read(writer: &mut EventStore, turn: u64, file: u64, cached: bool) {
        writer
            .append(EventPayload::TurnStarted {
                ts: "ts".into(),
                conversation: "main".into(),
                turn,
            })
            .unwrap();
        writer.append(EventPayload::ToolResult { ts: "ts".into(), conversation: Some("main".into()), turn, tool_call_id: format!("read-{turn}"), status: "ok".into(), summary: "read".into(), model_content: "model view".into(), truncated: false,
            files_read: Vec::new(), files_changed: Vec::new(), commands_run: Vec::new(), memory_changed: None,
            structured: Some(json!({"kind":"file_content","canonical_path":format!("/fixture/{file}.txt"),"start_line":1,"raw_text":"x".repeat(4096),"cached":cached})) }).unwrap();
    }

    #[test]
    fn startup_keeps_request_counters_and_pending_arguments_without_response_bodies() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut writer = EventStore::open(&path).unwrap();
        writer
            .append(EventPayload::TurnStarted {
                turn: 1,
                ts: String::new(),
                conversation: "reviewer".into(),
            })
            .unwrap();
        writer
            .append(EventPayload::ModelResponse {
                conversation: None,
                turn: 1,
                ts: String::new(),
                request_id: "req_3".into(),
                text: "x".repeat(65_536),
                thinking: Some("y".repeat(65_536)),
                stop_reason: Some(crate::event::ModelStopReason::ToolUse),
                input_tokens_total: Some(100),
                output_tokens_total: Some(100),
            })
            .unwrap();
        writer
            .append(EventPayload::ToolCall {
                turn: 1,
                ts: String::new(),
                conversation: None,
                tool_call_id: "pending".into(),
                request_id: "req_3".into(),
                index: 0,
                tool: "run_command".into(),
                args: json!({"command":"echo exact"}),
            })
            .unwrap();
        writer
            .append(EventPayload::TurnStarted {
                turn: 2,
                ts: String::new(),
                conversation: "main".into(),
            })
            .unwrap();
        writer
            .append(EventPayload::ModelResponse {
                conversation: Some("main".into()),
                turn: 2,
                ts: String::new(),
                request_id: "req_9".into(),
                text: "z".repeat(65_536),
                thinking: None,
                stop_reason: None,
                input_tokens_total: None,
                output_tokens_total: None,
            })
            .unwrap();
        let events = read_agent_events(&path, "reviewer", 1024).unwrap();
        assert_eq!(3, events.len());
        assert!(events.iter().any(|event| matches!(&event.payload,
            EventPayload::ModelResponse { conversation: Some(agent), request_id, text, thinking, .. }
            if agent == "reviewer" && request_id == "req_3" && text.is_empty() && thinking.is_none()
        )));
        assert!(events.iter().any(|event| matches!(&event.payload,
            EventPayload::ToolCall { conversation: Some(agent), args, .. }
            if agent == "reviewer" && args["command"] == "echo exact"
        )));
    }

    #[test]
    fn startup_read_snapshots_follow_both_byte_and_recent_access_limits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut writer = EventStore::open(&path).unwrap();
        for file in 0..130 {
            append_read(&mut writer, file + 1, file, false);
        }
        let generous = read_agent_events(&path, "main", 1024 * 1024).unwrap();
        let snapshots = |events: &[StoredEvent]| {
            events
                .iter()
                .filter_map(|event| match &event.payload {
                    EventPayload::ToolResult {
                        structured: Some(value),
                        ..
                    } if value["kind"] == "file_content" => {
                        value["canonical_path"].as_str().map(str::to_string)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(MAX_CACHED_READS, snapshots(&generous).len());
        append_read(&mut writer, 131, 127, true);
        append_read(&mut writer, 132, 130, false);
        let bounded = read_agent_events(&path, "main", 4096).unwrap();
        assert!(serde_json::to_vec(&bounded).unwrap().len() <= 4096 * MAX_SELECTION_BYTES_FACTOR);
        let paths = snapshots(&bounded);
        assert!(paths.contains(&"/fixture/127.txt".into()));
        assert!(paths.contains(&"/fixture/130.txt".into()));
        assert!(!paths.contains(&"/fixture/128.txt".into()));
    }
}
