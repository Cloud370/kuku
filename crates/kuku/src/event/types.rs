use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub id: u64,
    #[serde(flatten)]
    pub payload: EventPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
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
        role: String,
        alias: String,
        resolved_provider: String,
        resolved_model: String,
        params: Value,
    },

    #[serde(rename = "model.response")]
    ModelResponse {
        turn: u64,
        ts: String,
        request_id: String,
        text: String,
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
