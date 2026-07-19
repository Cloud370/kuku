use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::context::provenance::{
    AgentRegistryProvenance, FileSource, PluginRegistryProvenance, PromptCapabilityMetadata,
    PromptRendererIdentity, SkillRegistryProvenance, ToolRegistryProvenance,
};

use super::TaskLedgerRecord;

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
    TaskLedger(TaskLedgerRecord),
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
            Self::TaskLedger(record) => {
                let encoded = serde_json::to_value(record).map_err(serde::ser::Error::custom)?;
                let record_type = encoded.get("record_type").cloned().unwrap_or(Value::Null);
                let content = encoded.get("record").cloned().unwrap_or(Value::Null);
                let value = serde_json::json!({
                    "kind": "task.ledger",
                    "record_type": record_type,
                    "record": content,
                });
                value.serialize(serializer)
            }
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

impl<'de> Deserialize<'de> for EventPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let Some(object) = value.as_object() else {
            return Ok(Self::Unknown(value));
        };
        Self::from_json_object(object)
            .ok_or_else(|| serde::de::Error::custom("invalid event payload"))
    }
}

impl EventPayload {
    /// Returns the stable persisted kind name for this event.
    pub fn kind_name(&self) -> &str {
        match self {
            Self::TaskLedger(_) => "task.ledger",
            Self::ContextSources { .. } => "context.sources",
            Self::ContextSkills { .. } => "context.skills",
            Self::ModelResponse { .. } => "model.response",
            Self::ModelError { .. } => "model.error",
            Self::ToolCall { .. } => "tool.call",
            Self::PermissionRequested { .. } => "permission.requested",
            Self::PermissionAllow { .. } => "permission.allow",
            Self::PermissionDeny { .. } => "permission.deny",
            Self::ToolResult { .. } => "tool.result",
            Self::Handoff { .. } => "handoff",
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

    /// Returns the stable type name for this event.
    pub fn type_name(&self) -> &str {
        self.kind_name()
    }
}
