use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::provenance::FileSource;
use crate::skill::registry::SkillRegistry;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A single event persisted in a session's events.jsonl.
pub struct StoredEvent {
    pub id: u64,
    #[serde(flatten)]
    pub payload: EventPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A single message in a frozen prelude snapshot.
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

/// Scope of a turn rollback operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RollbackScope {
    #[serde(rename = "conversation_only")]
    ConversationOnly,
    #[serde(rename = "files_only")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
/// All fact events that can be written to and read from a session's events.jsonl.
pub enum EventPayload {
    #[serde(rename = "session.meta")]
    SessionMeta {
        ts: String,
        schema_version: u32,
        session_id: String,
        created_at: String,
        kuku_version: String,
    },

    #[serde(rename = "context.prelude")]
    ContextPrelude {
        ts: String,
        messages: Vec<ContextMessage>,
    },

    #[serde(rename = "context.sources")]
    ContextSources {
        turn: u64,
        ts: String,
        request_id: String,
        project_instruction_sources: Vec<FileSource>,
        memory_sources: Vec<FileSource>,
    },

    #[serde(rename = "context.skills")]
    ContextSkills {
        turn: u64,
        ts: String,
        registry: SkillRegistry,
        bootstrap_loaded: Vec<String>,
    },

    #[serde(rename = "turn.start")]
    TurnStart { turn: u64, ts: String },

    #[serde(rename = "user.input")]
    UserInput { turn: u64, ts: String, text: String },

    #[serde(rename = "model.response")]
    ModelResponse {
        turn: u64,
        ts: String,
        request_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_tokens_total: Option<u32>,
    },

    #[serde(rename = "model.error")]
    ModelError {
        turn: u64,
        ts: String,
        request_id: String,
        kind: String,
        message: String,
    },

    #[serde(rename = "tool.call")]
    ToolCall {
        turn: u64,
        ts: String,
        tool_call_id: String,
        request_id: String,
        index: u64,
        tool: String,
        args: Value,
    },

    #[serde(rename = "permission.allow")]
    PermissionAllow {
        turn: u64,
        ts: String,
        tool_call_id: String,
        tool: String,
        scope: String,
        matcher: String,
        source: String,
    },

    #[serde(rename = "permission.requested")]
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

    #[serde(rename = "permission.deny")]
    PermissionDeny {
        turn: u64,
        ts: String,
        tool_call_id: String,
        tool: String,
        reason: String,
        source: String,
    },

    #[serde(rename = "tool.result")]
    ToolResult {
        turn: u64,
        ts: String,
        tool_call_id: String,
        status: String,
        summary: String,
        model_content: String,
        truncated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        structured: Option<Value>,
    },

    #[serde(rename = "handoff")]
    Handoff {
        turn: u64,
        ts: String,
        request_id: String,
        summary: String,
        keep_turns: usize,
    },

    #[serde(rename = "turn.end")]
    TurnEnd { turn: u64, ts: String },

    #[serde(rename = "turn.rollback")]
    TurnRollback {
        turn: u64,
        ts: String,
        target_turn: u64,
        scope: RollbackScope,
    },

    #[serde(rename = "turn.rollback.undo")]
    TurnRollbackUndo {
        turn: u64,
        ts: String,
        rollback_event_id: u64,
    },

    /// Unknown event type — raw JSON preserved for display, excluded from messages[].
    /// Not deserialized by serde; created manually in two-step deserialization.
    #[serde(skip)]
    Unknown(Value),
}

impl EventPayload {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::SessionMeta { .. } => "session.meta",
            Self::ContextPrelude { .. } => "context.prelude",
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
            Self::TurnEnd { .. } => "turn.end",
            Self::TurnRollback { .. } => "turn.rollback",
            Self::TurnRollbackUndo { .. } => "turn.rollback.undo",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_prelude_round_trip() {
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
        let json = serde_json::to_string(&event).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
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
        assert_eq!(json["type"], "handoff");
    }

    #[test]
    fn rollback_scope_variants_serialize_correctly() {
        let cases = [
            (RollbackScope::ConversationOnly, r#""conversation_only""#),
            (RollbackScope::FilesOnly, r#""files_only""#),
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
    fn turn_rollback_undo_round_trip() {
        let event = StoredEvent {
            id: 51,
            payload: EventPayload::TurnRollbackUndo {
                turn: 6,
                ts: "2026-05-28T00:01:00Z".to_string(),
                rollback_event_id: 50,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn turn_rollback_event_type_tag() {
        let event = StoredEvent {
            id: 1,
            payload: EventPayload::TurnRollback {
                turn: 1,
                ts: "t".to_string(),
                target_turn: 1,
                scope: RollbackScope::ConversationOnly,
            },
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap()["type"],
            "turn.rollback"
        );
    }
}
