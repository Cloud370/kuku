use crate::context::{CanonicalMessage, MessageBlock, ToolResult, ToolUse};
use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, RollbackScope, StoredEvent};
use serde_json::json;

use super::rebuild_history;
use crate::context::revert::filter_rolled_back_events;

fn event(id: u64, payload: EventPayload) -> StoredEvent {
    StoredEvent { id, payload }
}

fn user_input(id: u64, turn: u64, text: &str) -> StoredEvent {
    event(
        id,
        EventPayload::MessageUser {
            execution: crate::event::test_execution_scope(),
            turn,
            ts: "2026-05-13T00:00:00Z".to_string(),
            conversation: "main".to_string(),
            text: text.to_string(),
            from: None,
            via_tool_call_id: None,
        },
    )
}

fn model_response(id: u64, turn: u64, request_id: &str, text: &str) -> StoredEvent {
    event(
        id,
        EventPayload::ModelResponse {
            turn,
            ts: "2026-05-13T00:00:01Z".to_string(),
            request: crate::event::test_request_scope(request_id),
            text: text.to_string(),
            thinking: None,
            input_tokens_total: Some(10),
        },
    )
}

fn tool_call(
    id: u64,
    turn: u64,
    request_id: &str,
    tool_call_id: &str,
    index: u64,
    tool: &str,
) -> StoredEvent {
    event(
        id,
        EventPayload::ToolCall {
            turn,
            ts: "2026-05-13T00:00:02Z".to_string(),
            conversation: None,
            tool_call_id: tool_call_id.to_string(),
            request: crate::event::test_request_scope(request_id),
            index,
            tool: tool.to_string(),
            args: json!({"name": tool}),
        },
    )
}

fn tool_result(id: u64, turn: u64, tool_call_id: &str, model_content: &str) -> StoredEvent {
    event(
        id,
        EventPayload::ToolResult {
            execution: crate::event::test_execution_scope(),
            turn,
            ts: "2026-05-13T00:00:03Z".to_string(),
            conversation: None,
            tool_call_id: tool_call_id.to_string(),
            status: "ok".to_string(),
            summary: format!("{tool_call_id} summary"),
            model_content: model_content.to_string(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: None,
        },
    )
}

fn turn_end(id: u64, turn: u64) -> StoredEvent {
    event(
        id,
        EventPayload::TurnCompleted {
            execution: crate::event::test_execution_scope(),
            turn,
            ts: "2026-05-13T00:00:04Z".to_string(),
            conversation: "main".to_string(),
        },
    )
}

include!("tests/history.rs");
include!("tests/rollback.rs");
