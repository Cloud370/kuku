use serde_json::json;

use crate::query::UiEvent;

/// Converts a UiEvent to its wire format representation.
///
/// All variants produce a `Some` value. `run_start` is not a UiEvent variant —
/// the server synthesizes it separately.
pub fn to_wire(event: &UiEvent) -> Option<serde_json::Value> {
    match event {
        UiEvent::TextDelta { text } => Some(json!({
            "type": "text",
            "content": text,
        })),
        UiEvent::ThinkingDelta { text } => Some(json!({
            "type": "thinking",
            "content": text,
        })),
        UiEvent::ToolCall {
            tool_call_id,
            tool,
            summary,
        } => Some(json!({
            "type": "tool_start",
            "id": tool_call_id,
            "name": tool,
            "summary": summary,
        })),
        UiEvent::ToolResult {
            tool_call_id,
            name,
            status,
            summary,
            ..
        } => Some(json!({
            "type": "tool_end",
            "id": tool_call_id,
            "name": name,
            "status": status,
            "summary": summary,
        })),
        UiEvent::PermissionRequested { request } => Some(json!({
            "type": "permission",
            "id": request.id,
            "tool": request.tool,
            "risk": request.risk,
            "summary": request.summary,
        })),
        UiEvent::Done { turn, usage, .. } => Some(json!({
            "type": "done",
            "turn": turn,
            "usage": usage,
        })),
        UiEvent::TurnStart { .. } => Some(json!({
            "type": "turn_start",
        })),
        UiEvent::Error { code, message } => Some(json!({
            "type": "error",
            "code": code,
            "message": message,
        })),
        UiEvent::ModelRequest { model, provider } => Some(json!({
            "type": "model_request",
            "model": model,
            "provider": provider,
        })),
        UiEvent::SubexecStart {
            stage_id,
            kind,
            label,
        } => Some(json!({
            "type": "subexec_start",
            "stage_id": stage_id,
            "kind": kind,
            "label": label,
        })),
        UiEvent::SubexecOutput { stage_id, event } => Some(json!({
            "type": "subexec_output",
            "stage_id": stage_id,
            "event": event,
        })),
        UiEvent::SubexecEnd {
            stage_id,
            status,
            summary,
            result,
        } => Some(json!({
            "type": "subexec_end",
            "stage_id": stage_id,
            "status": status,
            "summary": summary,
            "result": result,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{PermissionRequest, RunOutput, UiEvent};

    #[test]
    fn text_delta_wire_format() {
        let event = UiEvent::TextDelta {
            text: "hello".to_string(),
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "text");
        assert_eq!(wire["content"], "hello");
    }

    #[test]
    fn thinking_delta_wire_format() {
        let event = UiEvent::ThinkingDelta {
            text: "reasoning".to_string(),
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "thinking");
        assert_eq!(wire["content"], "reasoning");
    }

    #[test]
    fn tool_call_wire_format() {
        let event = UiEvent::ToolCall {
            tool_call_id: "tc_1".to_string(),
            tool: "find_files".to_string(),
            summary: "path: \".\"".to_string(),
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "tool_start");
        assert_eq!(wire["id"], "tc_1");
        assert_eq!(wire["name"], "find_files");
        assert_eq!(wire["summary"], "path: \".\"");
    }

    #[test]
    fn tool_result_wire_format() {
        let event = UiEvent::ToolResult {
            tool_call_id: "tc_1".to_string(),
            name: "find_files".to_string(),
            status: "ok".to_string(),
            summary: "found 3 files".to_string(),
            structured: None,
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "tool_end");
        assert_eq!(wire["id"], "tc_1");
        assert_eq!(wire["name"], "find_files");
        assert_eq!(wire["status"], "ok");
        assert_eq!(wire["summary"], "found 3 files");
    }

    #[test]
    fn permission_wire_format() {
        let event = UiEvent::PermissionRequested {
            request: PermissionRequest {
                id: "req_1".to_string(),
                tool_call_id: "tc_1".to_string(),
                tool: "run_command".to_string(),
                risk: "command".to_string(),
                summary: "cargo test".to_string(),
            },
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "permission");
        assert_eq!(wire["id"], "req_1");
        assert_eq!(wire["tool"], "run_command");
        assert_eq!(wire["risk"], "command");
        assert_eq!(wire["summary"], "cargo test");
    }

    #[test]
    fn done_wire_format() {
        let event = UiEvent::Done {
            output: RunOutput {
                session_id: "s1".to_string(),
                text: "done".to_string(),
                usage: None,
                turn: 1,
            },
            usage: None,
            turn: 1,
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "done");
        assert_eq!(wire["turn"], 1);
    }

    #[test]
    fn turn_start_wire_format() {
        let event = UiEvent::TurnStart { turn: 1 };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "turn_start");
    }

    #[test]
    fn error_wire_format() {
        let event = UiEvent::Error {
            code: "provider_rate_limit".to_string(),
            message: "rate limited".to_string(),
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "error");
        assert_eq!(wire["code"], "provider_rate_limit");
        assert_eq!(wire["message"], "rate limited");
    }

    #[test]
    fn model_request_wire_format() {
        let event = UiEvent::ModelRequest {
            model: "claude-sonnet-4-6".to_string(),
            provider: "anthropic".to_string(),
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "model_request");
        assert_eq!(wire["model"], "claude-sonnet-4-6");
        assert_eq!(wire["provider"], "anthropic");
    }
}
