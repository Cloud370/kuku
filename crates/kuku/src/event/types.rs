use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::context::provenance::{
    AgentRegistryProvenance, FileSource, PluginRegistryProvenance, PromptCapabilityMetadata,
    PromptRendererIdentity, SkillRegistryProvenance, ToolRegistryProvenance,
};
/// A single event persisted in a session's events.jsonl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub id: u64,
    pub payload: EventPayload,
}

impl Serialize for StoredEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.payload {
            EventPayload::Unknown(value) => value.serialize(serializer),
            payload => {
                let value = payload
                    .to_new_json(self.id)
                    .map_err(serde::ser::Error::custom)?;
                value.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for StoredEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| de::Error::custom("stored event must be a JSON object"))?;
        let id = object
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| de::Error::custom("stored event is missing numeric id"))?;

        match EventPayload::from_json_object(object) {
            Some(payload) => Ok(Self { id, payload }),
            None => Ok(Self {
                id,
                payload: EventPayload::Unknown(value),
            }),
        }
    }
}

/// A single message in a frozen prelude snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

/// Scope of a turn rollback operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RollbackScope {
    #[serde(rename = "messages")]
    ConversationOnly,
    #[serde(rename = "file_changes")]
    FilesOnly,
    #[serde(rename = "both")]
    Both,
}

impl RollbackScope {
    /// Whether this scope skips conversation events during rebuild.
    pub fn affects_conversation(&self) -> bool {
        matches!(self, Self::ConversationOnly | Self::Both)
    }

    /// Whether this scope triggers file revert operations.
    pub fn affects_files(&self) -> bool {
        matches!(self, Self::FilesOnly | Self::Both)
    }
}

/// All fact events that can be written to and read from a session's events.jsonl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventPayload {
    SessionMeta {
        ts: String,
        schema_version: u32,
        session_id: String,
        created_at: String,
        kuku_version: String,
    },
    ContextPrelude {
        ts: String,
        messages: Vec<ContextMessage>,
    },
    ContextSources {
        turn: u64,
        ts: String,
        request_id: String,
        project_instruction_sources: Vec<FileSource>,
        memory_sources: Vec<FileSource>,
    },
    ContextSkills {
        conversation: String,
        turn: u64,
        ts: String,
        registry: Value,
        bootstrap_loaded: Vec<String>,
    },
    TurnStart {
        turn: u64,
        ts: String,
    },
    UserInput {
        turn: u64,
        ts: String,
        text: String,
    },
    ModelResponse {
        turn: u64,
        ts: String,
        request_id: String,
        text: String,
        thinking: Option<String>,
        input_tokens_total: Option<u32>,
    },
    ModelError {
        turn: u64,
        ts: String,
        request_id: String,
        kind: String,
        message: String,
    },
    ToolCall {
        turn: u64,
        ts: String,
        conversation: Option<String>,
        tool_call_id: String,
        request_id: String,
        index: u64,
        tool: String,
        args: Value,
    },
    PermissionAllow {
        turn: u64,
        ts: String,
        tool_call_id: String,
        tool: String,
        scope: String,
        matcher: String,
        source: String,
    },
    PermissionRequested {
        turn: u64,
        ts: String,
        tool_call_id: String,
        tool: String,
        risk: String,
        summary: String,
        candidate: String,
        source: String,
    },
    PermissionDeny {
        turn: u64,
        ts: String,
        tool_call_id: String,
        tool: String,
        reason: String,
        source: String,
    },
    ToolResult {
        turn: u64,
        ts: String,
        conversation: Option<String>,
        tool_call_id: String,
        status: String,
        summary: String,
        model_content: String,
        truncated: bool,
        files_read: Vec<String>,
        files_changed: Vec<String>,
        commands_run: Vec<String>,
        memory_changed: Option<Value>,
        structured: Option<Value>,
    },
    Handoff {
        turn: u64,
        ts: String,
        request_id: String,
        summary: String,
        keep_turns: usize,
    },
    TurnEnd {
        turn: u64,
        ts: String,
    },
    TurnRollback {
        turn: u64,
        ts: String,
        target_turn: u64,
        scope: RollbackScope,
    },
    TurnRollbackUndo {
        turn: u64,
        ts: String,
        rollback_event_id: u64,
    },
    SessionCreated {
        ts: String,
        schema_version: u32,
        session_id: String,
        created_at: String,
        kuku_version: String,
    },
    ConversationOpened {
        ts: String,
        conversation: String,
    },
    ConversationBound {
        ts: String,
        conversation: String,
        binding_id: String,
    },
    PromptSnapshot {
        ts: String,
        conversation: String,
        binding_id: String,
        snapshot_id: String,
        turn: u64,
        messages: Vec<ContextMessage>,
        project_instruction_sources: Vec<FileSource>,
        memory_sources: Vec<FileSource>,
        prompt_asset_sources: Vec<FileSource>,
        skills: Value,
        bootstrap_loaded: Vec<String>,
        provider: String,
        model: String,
        renderer: PromptRendererIdentity,
        tool_registry: Box<ToolRegistryProvenance>,
        agent_registry: Option<AgentRegistryProvenance>,
        skill_registry: Box<Option<SkillRegistryProvenance>>,
        plugin_registry: Box<Option<PluginRegistryProvenance>>,
        capabilities: PromptCapabilityMetadata,
    },
    MessageUser {
        ts: String,
        conversation: String,
        turn: u64,
        text: String,
        from: Option<String>,
        via_tool_call_id: Option<String>,
    },
    MessageAssistant {
        ts: String,
        conversation: String,
        turn: u64,
        message_id: String,
        text: String,
    },
    TurnStarted {
        ts: String,
        conversation: String,
        turn: u64,
    },
    TurnCompleted {
        ts: String,
        conversation: String,
        turn: u64,
    },
    TurnCancelled {
        ts: String,
        conversation: String,
        turn: u64,
        reason: String,
    },
    TurnInterrupted {
        ts: String,
        conversation: String,
        turn: u64,
        reason: String,
    },
    ConversationRollback {
        ts: String,
        conversation: String,
        to_turn: u64,
        to_event_id: u64,
        scope: RollbackScope,
    },
    ConversationRollbackUndone {
        ts: String,
        conversation: String,
        rollback_event_id: u64,
    },
    Unknown(Value),
}

impl Serialize for EventPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Unknown(value) => value.serialize(serializer),
            payload => {
                let mut value = payload.to_new_json(0).map_err(serde::ser::Error::custom)?;
                if let Some(object) = value.as_object_mut() {
                    object.remove("id");
                }
                value.serialize(serializer)
            }
        }
    }
}

impl EventPayload {
    pub fn kind_name(&self) -> &str {
        match self {
            Self::SessionMeta { .. } => "session.created",
            Self::ContextPrelude { .. } => "prompt.snapshot",
            Self::ContextSources { .. } => "context.sources",
            Self::ContextSkills { .. } => "context.skills",
            Self::TurnStart { .. } => "turn.start",
            Self::UserInput { .. } => "user.input",
            Self::ModelResponse { .. } => "model.response",
            Self::ModelError { .. } => "model.error",
            Self::ToolCall { .. } => "tool.call",
            Self::PermissionRequested { .. } => "permission.requested",
            Self::PermissionAllow { .. } => "permission.allow",
            Self::PermissionDeny { .. } => "permission.deny",
            Self::ToolResult { .. } => "tool.result",
            Self::Handoff { .. } => "handoff",
            Self::TurnEnd { .. } => "turn.completed",
            Self::TurnRollback { .. } => "turn.rollback",
            Self::TurnRollbackUndo { .. } => "turn.rollback.undo",
            Self::SessionCreated { .. } => "session.created",
            Self::ConversationOpened { .. } => "conversation.opened",
            Self::ConversationBound { .. } => "conversation.bound",
            Self::PromptSnapshot { .. } => "prompt.snapshot",
            Self::MessageUser { .. } => "message.user",
            Self::MessageAssistant { .. } => "message.assistant",
            Self::TurnStarted { .. } => "turn.started",
            Self::TurnCompleted { .. } => "turn.completed",
            Self::TurnCancelled { .. } => "turn.cancelled",
            Self::TurnInterrupted { .. } => "turn.interrupted",
            Self::ConversationRollback { .. } => "conversation.rollback",
            Self::ConversationRollbackUndone { .. } => "conversation.rollback.undone",
            Self::Unknown(value) => value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        }
    }

    pub fn type_name(&self) -> &str {
        self.kind_name()
    }

    fn from_json_object(object: &Map<String, Value>) -> Option<Self> {
        let kind = object.get("kind").and_then(Value::as_str)?;
        let value = Value::Object(object.clone());
        match kind {
            "context.sources" => Some(Self::ContextSources {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                request_id: string_field(object, "request_id")?,
                project_instruction_sources: serde_json::from_value(
                    object.get("project_instruction_sources")?.clone(),
                )
                .ok()?,
                memory_sources: serde_json::from_value(object.get("memory_sources")?.clone())
                    .ok()?,
            }),
            "context.skills" => Some(Self::ContextSkills {
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                registry: object.get("registry")?.clone(),
                bootstrap_loaded: serde_json::from_value(object.get("bootstrap_loaded")?.clone())
                    .ok()?,
            }),
            "user.input" => Some(Self::UserInput {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                text: string_field(object, "text")?,
            }),
            "model.response" => Some(Self::ModelResponse {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                request_id: string_field(object, "request_id")?,
                text: string_field(object, "text")?,
                thinking: optional_string_field(object, "thinking"),
                input_tokens_total: optional_u32_field(object, "input_tokens_total"),
            }),
            "model.error" => Some(Self::ModelError {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                request_id: string_field(object, "request_id")?,
                kind: string_field(object, "error_kind")
                    .or_else(|| string_field(object, "kind"))?,
                message: string_field(object, "message")?,
            }),
            "tool.call" => Some(Self::ToolCall {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                conversation: optional_string_field(object, "conversation"),
                tool_call_id: string_field(object, "tool_call_id")?,
                request_id: string_field(object, "request_id")?,
                index: u64_field(object, "index")?,
                tool: string_field(object, "tool")?,
                args: object.get("args")?.clone(),
            }),
            "permission.allow" => Some(Self::PermissionAllow {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                tool_call_id: string_field(object, "tool_call_id")?,
                tool: string_field(object, "tool")?,
                scope: string_field(object, "scope")?,
                matcher: string_field(object, "matcher")?,
                source: string_field(object, "source")?,
            }),
            "permission.requested" => Some(Self::PermissionRequested {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                tool_call_id: string_field(object, "tool_call_id")?,
                tool: string_field(object, "tool")?,
                risk: string_field(object, "risk")?,
                summary: string_field(object, "summary")?,
                candidate: string_field(object, "candidate")?,
                source: string_field(object, "source")?,
            }),
            "permission.deny" => Some(Self::PermissionDeny {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                tool_call_id: string_field(object, "tool_call_id")?,
                tool: string_field(object, "tool")?,
                reason: string_field(object, "reason")?,
                source: string_field(object, "source")?,
            }),
            "tool.result" => Some(Self::ToolResult {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                conversation: optional_string_field(object, "conversation"),
                tool_call_id: string_field(object, "tool_call_id")?,
                status: string_field(object, "status")?,
                summary: string_field(object, "summary")?,
                model_content: string_field(object, "model_content")?,
                truncated: bool_field(object, "truncated")?,
                files_read: vec_string_field(object, "files_read"),
                files_changed: vec_string_field(object, "files_changed"),
                commands_run: vec_string_field(object, "commands_run"),
                memory_changed: object.get("memory_changed").cloned(),
                structured: object.get("structured").cloned(),
            }),
            "handoff" => Some(Self::Handoff {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                request_id: string_field(object, "request_id")?,
                summary: string_field(object, "summary")?,
                keep_turns: usize_field(object, "keep_turns")?,
            }),
            "turn.rollback" => Some(Self::TurnRollback {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                target_turn: u64_field(object, "target_turn")?,
                scope: serde_json::from_value(object.get("scope")?.clone()).ok()?,
            }),
            "turn.rollback.undo" => Some(Self::TurnRollbackUndo {
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                rollback_event_id: u64_field(object, "rollback_event_id")?,
            }),
            "session.created" => Some(Self::SessionCreated {
                ts: string_field(object, "ts")?,
                schema_version: u32_field(object, "schema_version")?,
                session_id: string_field(object, "session_id")?,
                created_at: string_field(object, "created_at")?,
                kuku_version: string_field(object, "kuku_version")?,
            }),
            "conversation.opened" => Some(Self::ConversationOpened {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
            }),
            "conversation.bound" => Some(Self::ConversationBound {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                binding_id: string_field(object, "binding_id")?,
            }),
            "prompt.snapshot" => Some(Self::PromptSnapshot {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                binding_id: string_field(object, "binding_id")?,
                snapshot_id: string_field(object, "snapshot_id")?,
                turn: u64_field(object, "turn")?,
                messages: serde_json::from_value(object.get("messages")?.clone()).ok()?,
                project_instruction_sources: serde_json::from_value(
                    object.get("project_instruction_sources")?.clone(),
                )
                .ok()?,
                memory_sources: serde_json::from_value(object.get("memory_sources")?.clone())
                    .ok()?,
                prompt_asset_sources: serde_json::from_value(
                    object.get("prompt_asset_sources")?.clone(),
                )
                .ok()?,
                skills: object.get("skills")?.clone(),
                bootstrap_loaded: serde_json::from_value(object.get("bootstrap_loaded")?.clone())
                    .ok()?,
                provider: string_field(object, "provider")?,
                model: string_field(object, "model")?,
                renderer: serde_json::from_value(object.get("renderer")?.clone()).ok()?,
                tool_registry: Box::new(
                    serde_json::from_value(object.get("tool_registry")?.clone()).ok()?,
                ),
                agent_registry: optional_json_field(object, "agent_registry")
                    .and_then(|value| serde_json::from_value(value).ok()),
                skill_registry: Box::new(
                    optional_json_field(object, "skill_registry")
                        .and_then(|value| serde_json::from_value(value).ok()),
                ),
                plugin_registry: Box::new(
                    optional_json_field(object, "plugin_registry")
                        .and_then(|value| serde_json::from_value(value).ok()),
                ),
                capabilities: serde_json::from_value(object.get("capabilities")?.clone()).ok()?,
            }),
            "message.user" => Some(Self::MessageUser {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                text: string_field(object, "text")?,
                from: optional_string_field(object, "from"),
                via_tool_call_id: optional_string_field(object, "via_tool_call_id"),
            }),
            "message.assistant" => Some(Self::MessageAssistant {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                message_id: string_field(object, "message_id")?,
                text: string_field(object, "text")?,
            }),
            "turn.started" => Some(Self::TurnStarted {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
            }),
            "turn.completed" => Some(Self::TurnCompleted {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
            }),
            "turn.cancelled" => Some(Self::TurnCancelled {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                reason: string_field(object, "reason")?,
            }),
            "turn.interrupted" => Some(Self::TurnInterrupted {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                reason: string_field(object, "reason")?,
            }),
            "conversation.rollback" => Some(Self::ConversationRollback {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                to_turn: u64_field(object, "to_turn")?,
                to_event_id: u64_field(object, "to_event_id")?,
                scope: serde_json::from_value(object.get("scope")?.clone()).ok()?,
            }),
            "conversation.rollback.undone" => Some(Self::ConversationRollbackUndone {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                rollback_event_id: u64_field(object, "rollback_event_id")?,
            }),
            _ => Some(Self::Unknown(value)),
        }
    }

    fn to_new_json(&self, id: u64) -> serde_json::Result<Value> {
        match self {
            Self::Unknown(value) => Ok(value.clone()),
            Self::SessionMeta {
                ts,
                schema_version,
                session_id,
                created_at,
                kuku_version,
            }
            | Self::SessionCreated {
                ts,
                schema_version,
                session_id,
                created_at,
                kuku_version,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "session.created",
                "schema_version": schema_version,
                "session_id": session_id,
                "created_at": created_at,
                "kuku_version": kuku_version,
            })),
            Self::ContextPrelude { ts, messages } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "prompt.snapshot",
                "conversation": "main",
                "binding_id": "binding:main",
                "snapshot_id": "snapshot:main:0",
                "turn": 0,
                "messages": messages,
                "project_instruction_sources": [],
                "memory_sources": [],
                "prompt_asset_sources": [],
                "skills": {"names": [], "hash": ""},
                "bootstrap_loaded": [],
                "provider": "",
                "model": "",
                "renderer": {"provider": "", "renderer": ""},
                "tool_registry": {"hash": "", "names": [], "tool_count": 0},
                "capabilities": {
                    "context_budget_tier": "",
                    "max_context_tokens": null,
                    "remaining_input_tokens": null
                },
            })),
            Self::ContextSources {
                turn,
                ts,
                request_id,
                project_instruction_sources,
                memory_sources,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "context.sources",
                "turn": turn,
                "request_id": request_id,
                "project_instruction_sources": project_instruction_sources,
                "memory_sources": memory_sources,
            })),
            Self::ContextSkills {
                conversation,
                turn,
                ts,
                registry,
                bootstrap_loaded,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "context.skills",
                "conversation": conversation,
                "turn": turn,
                "registry": registry,
                "bootstrap_loaded": bootstrap_loaded,
            })),
            Self::TurnStart { turn, ts } | Self::TurnStarted { turn, ts, .. } => {
                let conversation = match self {
                    Self::TurnStarted { conversation, .. } => Some(conversation.as_str()),
                    _ => None,
                };
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert(
                    "kind".into(),
                    Value::from(if conversation.is_some() {
                        "turn.started"
                    } else {
                        "turn.start"
                    }),
                );
                map.insert("turn".into(), Value::from(*turn));
                if let Some(conversation) = conversation {
                    map.insert("conversation".into(), Value::from(conversation));
                }
                Ok(Value::Object(map))
            }
            Self::UserInput { turn, ts, text } | Self::MessageUser { turn, ts, text, .. } => {
                let (conversation, from, via_tool_call_id) = match self {
                    Self::MessageUser {
                        conversation,
                        from,
                        via_tool_call_id,
                        ..
                    } => (
                        Some(conversation.as_str()),
                        from.as_deref(),
                        via_tool_call_id.as_deref(),
                    ),
                    _ => (None, None, None),
                };
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert(
                    "kind".into(),
                    Value::from(if conversation.is_some() {
                        "message.user"
                    } else {
                        "user.input"
                    }),
                );
                map.insert("turn".into(), Value::from(*turn));
                map.insert("text".into(), Value::from(text.clone()));
                if let Some(conversation) = conversation {
                    map.insert("conversation".into(), Value::from(conversation));
                }
                if let Some(from) = from {
                    map.insert("from".into(), Value::from(from));
                }
                if let Some(via_tool_call_id) = via_tool_call_id {
                    map.insert("via_tool_call_id".into(), Value::from(via_tool_call_id));
                }
                Ok(Value::Object(map))
            }
            Self::ModelResponse {
                turn,
                ts,
                request_id,
                text,
                thinking,
                input_tokens_total,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("model.response"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("request_id".into(), Value::from(request_id.clone()));
                map.insert("text".into(), Value::from(text.clone()));
                if let Some(thinking) = thinking {
                    map.insert("thinking".into(), Value::from(thinking.clone()));
                }
                if let Some(input_tokens_total) = input_tokens_total {
                    map.insert(
                        "input_tokens_total".into(),
                        Value::from(*input_tokens_total),
                    );
                }
                Ok(Value::Object(map))
            }
            Self::ModelError {
                turn,
                ts,
                request_id,
                kind,
                message,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "model.error",
                "turn": turn,
                "request_id": request_id,
                "error_kind": kind,
                "message": message,
            })),
            Self::ToolCall {
                turn,
                ts,
                conversation,
                tool_call_id,
                request_id,
                index,
                tool,
                args,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("tool.call"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("tool_call_id".into(), Value::from(tool_call_id.clone()));
                map.insert("request_id".into(), Value::from(request_id.clone()));
                map.insert("index".into(), Value::from(*index));
                map.insert("tool".into(), Value::from(tool.clone()));
                map.insert("args".into(), args.clone());
                if let Some(conversation) = conversation.as_ref() {
                    map.insert("conversation".into(), Value::from(conversation.clone()));
                }
                Ok(Value::Object(map))
            }
            Self::PermissionAllow {
                turn,
                ts,
                tool_call_id,
                tool,
                scope,
                matcher,
                source,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "permission.allow",
                "turn": turn,
                "tool_call_id": tool_call_id,
                "tool": tool,
                "scope": scope,
                "matcher": matcher,
                "source": source,
            })),
            Self::PermissionRequested {
                turn,
                ts,
                tool_call_id,
                tool,
                risk,
                summary,
                candidate,
                source,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "permission.requested",
                "turn": turn,
                "tool_call_id": tool_call_id,
                "tool": tool,
                "risk": risk,
                "summary": summary,
                "candidate": candidate,
                "source": source,
            })),
            Self::PermissionDeny {
                turn,
                ts,
                tool_call_id,
                tool,
                reason,
                source,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "permission.deny",
                "turn": turn,
                "tool_call_id": tool_call_id,
                "tool": tool,
                "reason": reason,
                "source": source,
            })),
            Self::ToolResult {
                turn,
                ts,
                conversation,
                tool_call_id,
                status,
                summary,
                model_content,
                truncated,
                files_read,
                files_changed,
                commands_run,
                memory_changed,
                structured,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("tool.result"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("tool_call_id".into(), Value::from(tool_call_id.clone()));
                map.insert("status".into(), Value::from(status.clone()));
                map.insert("summary".into(), Value::from(summary.clone()));
                map.insert("model_content".into(), Value::from(model_content.clone()));
                map.insert("truncated".into(), Value::from(*truncated));
                if let Some(conversation) = conversation.as_ref() {
                    map.insert("conversation".into(), Value::from(conversation.clone()));
                }
                if !files_read.is_empty() {
                    map.insert("files_read".into(), serde_json::to_value(files_read)?);
                }
                if !files_changed.is_empty() {
                    map.insert("files_changed".into(), serde_json::to_value(files_changed)?);
                }
                if !commands_run.is_empty() {
                    map.insert("commands_run".into(), serde_json::to_value(commands_run)?);
                }
                if let Some(memory_changed) = memory_changed {
                    map.insert("memory_changed".into(), memory_changed.clone());
                }
                if let Some(structured) = structured {
                    map.insert("structured".into(), structured.clone());
                }
                Ok(Value::Object(map))
            }
            Self::Handoff {
                turn,
                ts,
                request_id,
                summary,
                keep_turns,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "handoff",
                "turn": turn,
                "request_id": request_id,
                "summary": summary,
                "keep_turns": keep_turns,
            })),
            Self::TurnEnd { turn, ts } | Self::TurnCompleted { turn, ts, .. } => {
                let conversation = match self {
                    Self::TurnCompleted { conversation, .. } => Some(conversation.as_str()),
                    _ => None,
                };
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("turn.completed"));
                map.insert("turn".into(), Value::from(*turn));
                if let Some(conversation) = conversation {
                    map.insert("conversation".into(), Value::from(conversation));
                }
                Ok(Value::Object(map))
            }
            Self::TurnRollback {
                turn,
                ts,
                target_turn,
                scope,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "turn.rollback",
                "turn": turn,
                "target_turn": target_turn,
                "scope": scope,
            })),
            Self::TurnRollbackUndo {
                turn,
                ts,
                rollback_event_id,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "turn.rollback.undo",
                "turn": turn,
                "rollback_event_id": rollback_event_id,
            })),
            Self::ConversationOpened { ts, conversation } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.opened",
                "conversation": conversation,
            })),
            Self::ConversationBound {
                ts,
                conversation,
                binding_id,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.bound",
                "conversation": conversation,
                "binding_id": binding_id,
            })),
            Self::PromptSnapshot {
                ts,
                conversation,
                binding_id,
                snapshot_id,
                turn,
                messages,
                project_instruction_sources,
                memory_sources,
                prompt_asset_sources,
                skills,
                bootstrap_loaded,
                provider,
                model,
                renderer,
                tool_registry,
                agent_registry,
                skill_registry,
                plugin_registry,
                capabilities,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "prompt.snapshot",
                "conversation": conversation,
                "binding_id": binding_id,
                "snapshot_id": snapshot_id,
                "turn": turn,
                "messages": messages,
                "project_instruction_sources": project_instruction_sources,
                "memory_sources": memory_sources,
                "prompt_asset_sources": prompt_asset_sources,
                "skills": skills,
                "bootstrap_loaded": bootstrap_loaded,
                "provider": provider,
                "model": model,
                "renderer": renderer,
                "tool_registry": tool_registry,
                "agent_registry": agent_registry,
            "skill_registry": skill_registry,
                "plugin_registry": plugin_registry,
                "capabilities": capabilities,
            })),
            Self::MessageAssistant {
                ts,
                conversation,
                turn,
                message_id,
                text,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "message.assistant",
                "conversation": conversation,
                "turn": turn,
                "message_id": message_id,
                "text": text,
            })),
            Self::TurnCancelled {
                ts,
                conversation,
                turn,
                reason,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "turn.cancelled",
                "conversation": conversation,
                "turn": turn,
                "reason": reason,
            })),
            Self::TurnInterrupted {
                ts,
                conversation,
                turn,
                reason,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "turn.interrupted",
                "conversation": conversation,
                "turn": turn,
                "reason": reason,
            })),
            Self::ConversationRollback {
                ts,
                conversation,
                to_turn,
                to_event_id,
                scope,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.rollback",
                "conversation": conversation,
                "to_turn": to_turn,
                "to_event_id": to_event_id,
                "scope": scope,
            })),
            Self::ConversationRollbackUndone {
                ts,
                conversation,
                rollback_event_id,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.rollback.undone",
                "conversation": conversation,
                "rollback_event_id": rollback_event_id,
            })),
        }
    }
}

fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(ToOwned::to_owned)
}

fn optional_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn optional_json_field(object: &Map<String, Value>, key: &str) -> Option<Value> {
    object.get(key).cloned().filter(|value| !value.is_null())
}

fn u64_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key)?.as_u64()
}

fn usize_field(object: &Map<String, Value>, key: &str) -> Option<usize> {
    usize::try_from(object.get(key)?.as_u64()?).ok()
}

fn u32_field(object: &Map<String, Value>, key: &str) -> Option<u32> {
    u32::try_from(object.get(key)?.as_u64()?).ok()
}

fn optional_u32_field(object: &Map<String, Value>, key: &str) -> Option<u32> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn bool_field(object: &Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key)?.as_bool()
}

fn vec_string_field(object: &Map<String, Value>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_prelude_serializes_as_prompt_snapshot() {
        let event = StoredEvent {
            id: 1,
            payload: EventPayload::ContextPrelude {
                ts: "2026-05-27T00:00:00Z".to_string(),
                messages: vec![ContextMessage {
                    role: "user".to_string(),
                    content: "<kuku_tool_guidance>use tools</kuku_tool_guidance>".to_string(),
                }],
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "prompt.snapshot");
    }

    #[test]
    fn handoff_round_trip() {
        let event = StoredEvent {
            id: 43,
            payload: EventPayload::Handoff {
                turn: 3,
                ts: "2026-05-27T00:00:01Z".to_string(),
                request_id: "req_3".to_string(),
                summary: "## Goal\nBuild feature X".to_string(),
                keep_turns: 2,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn handoff_event_type_tag_is_handoff() {
        let event = StoredEvent {
            id: 1,
            payload: EventPayload::Handoff {
                turn: 1,
                ts: "t".to_string(),
                request_id: "req_1".to_string(),
                summary: "s".to_string(),
                keep_turns: 0,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "handoff");
    }

    #[test]
    fn rollback_scope_variants_serialize_correctly() {
        let cases = [
            (RollbackScope::ConversationOnly, r#""messages""#),
            (RollbackScope::FilesOnly, r#""file_changes""#),
            (RollbackScope::Both, r#""both""#),
        ];
        for (variant, expected) in &cases {
            assert_eq!(serde_json::to_string(variant).unwrap(), *expected);
            let back: RollbackScope = serde_json::from_str(expected).unwrap();
            assert_eq!(back, *variant);
        }
    }

    #[test]
    fn turn_rollback_round_trip() {
        let event = StoredEvent {
            id: 50,
            payload: EventPayload::TurnRollback {
                turn: 5,
                ts: "2026-05-28T00:00:00Z".to_string(),
                target_turn: 3,
                scope: RollbackScope::Both,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn conversation_rollback_round_trip() {
        let event = StoredEvent {
            id: 51,
            payload: EventPayload::ConversationRollback {
                ts: "2026-05-28T00:01:00Z".to_string(),
                conversation: "main".to_string(),
                to_turn: 3,
                to_event_id: 9,
                scope: RollbackScope::Both,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn conversation_rollback_undo_round_trip() {
        let event = StoredEvent {
            id: 1,
            payload: EventPayload::ConversationRollbackUndone {
                ts: "t".to_string(),
                conversation: "main".to_string(),
                rollback_event_id: 9,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn conversation_rollback_event_type_tag() {
        let event = StoredEvent {
            id: 1,
            payload: EventPayload::ConversationRollback {
                ts: "t".to_string(),
                conversation: "main".to_string(),
                to_turn: 1,
                to_event_id: 3,
                scope: RollbackScope::ConversationOnly,
            },
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap()["kind"],
            "conversation.rollback"
        );
    }
}
