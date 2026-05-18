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
    pub prelude: Vec<ContextMessage>,
    pub notices: Vec<ContextMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A single message in the rendered context snapshot.
pub struct ContextMessage {
    pub role: String,
    pub content: String,
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
}
