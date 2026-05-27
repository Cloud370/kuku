use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A single event persisted in a session's events.jsonl.
pub struct StoredEvent {
    pub id: u64,
    #[serde(flatten)]
    pub payload: EventPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// History metadata attached to a model request event.
pub struct RequestHistory {
    pub first: Option<u64>,
    pub last: Option<u64>,
    pub message_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Tool registry metadata attached to a model request event.
pub struct RequestTools {
    pub hash: Option<String>,
    pub count: Option<usize>,
    pub names: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Rendered context snapshot attached to a model request event.
pub struct RequestContext {
    pub system: String,
    pub prelude: Option<Vec<ContextMessage>>,
    pub notices: Vec<ContextMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A single message in the rendered context snapshot.
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

/// Reason for triggering a context handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandoffTriggerReason {
    #[serde(rename = "context_threshold")]
    ContextThreshold,
    #[serde(rename = "overflow_error")]
    OverflowError,
    /// Manual handoff requested by the user.
    #[serde(rename = "user")]
    User,
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
// ModelRequest is the largest variant; boxing adds indirection with no benefit for a serialization data enum.
#[allow(clippy::large_enum_variant)]
/// All event types that can be written to and read from a session's events.jsonl.
pub enum EventPayload {
    #[serde(rename = "session.meta")]
    SessionMeta {
        ts: String,
        schema_version: u32,
        session_id: String,
        created_at: String,
        kuku_version: String,
    },

    #[serde(rename = "turn.start")]
    TurnStart { turn: u64, ts: String },

    #[serde(rename = "user.input")]
    UserInput { turn: u64, ts: String, text: String },

    #[serde(rename = "model.request")]
    ModelRequest {
        turn: u64,
        ts: String,
        request_id: String,
        tier: String,
        think: String,
        provider: String,
        model: String,
        request_params: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history: Option<RequestHistory>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<RequestTools>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<RequestContext>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provenance: Option<Value>,
    },

    #[serde(rename = "model.response")]
    ModelResponse {
        turn: u64,
        ts: String,
        request_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        stop_reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_count: Option<u64>,
        usage: Value,
    },

    #[serde(rename = "model.error")]
    ModelError {
        turn: u64,
        ts: String,
        request_id: String,
        kind: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
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

    #[serde(rename = "policy.loaded")]
    PolicyLoaded {
        ts: String,
        policy_hash: String,
        mode: String,
    },

    #[serde(rename = "permission.request")]
    PermissionRequest {
        turn: u64,
        ts: String,
        tool_call_id: String,
        tool: String,
        risk: String,
        summary: String,
    },

    #[serde(rename = "permission.decision")]
    PermissionDecision {
        turn: u64,
        ts: String,
        tool_call_id: String,
        decision: String,
        scope: String,
        source: String,
        rule: String,
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

    #[serde(rename = "turn.end")]
    TurnEnd { turn: u64, ts: String },

    #[serde(rename = "handoff.trigger")]
    HandoffTrigger {
        ts: String,
        trigger: HandoffTriggerReason,
    },

    #[serde(rename = "handoff")]
    Handoff {
        ts: String,
        summary: String,
        kept_turns: usize,
    },

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_trigger_round_trip() {
        let event = StoredEvent {
            id: 42,
            payload: EventPayload::HandoffTrigger {
                ts: "2026-05-27T00:00:00Z".to_string(),
                trigger: HandoffTriggerReason::ContextThreshold,
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
                ts: "2026-05-27T00:00:01Z".to_string(),
                summary: "## Goal\nBuild feature X".to_string(),
                kept_turns: 2,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn handoff_trigger_reason_variants_serialize_correctly() {
        let cases = [
            (
                HandoffTriggerReason::ContextThreshold,
                r#""context_threshold""#,
            ),
            (HandoffTriggerReason::OverflowError, r#""overflow_error""#),
            (HandoffTriggerReason::User, r#""user""#),
        ];
        for (variant, expected) in &cases {
            assert_eq!(serde_json::to_string(variant).unwrap(), *expected);
            let back: HandoffTriggerReason = serde_json::from_str(expected).unwrap();
            assert_eq!(back, *variant);
        }
    }

    #[test]
    fn handoff_event_type_tag_is_handoff() {
        let event = StoredEvent {
            id: 1,
            payload: EventPayload::Handoff {
                ts: "t".to_string(),
                summary: "s".to_string(),
                kept_turns: 0,
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

    #[test]
    fn turn_rollback_undo_event_type_tag() {
        let event = StoredEvent {
            id: 1,
            payload: EventPayload::TurnRollbackUndo {
                turn: 1,
                ts: "t".to_string(),
                rollback_event_id: 0,
            },
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap()["type"],
            "turn.rollback.undo"
        );
    }
}
