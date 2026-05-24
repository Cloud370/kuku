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
        UiEvent::ToolStart {
            id,
            tool,
            summary,
            kind,
        } => {
            let mut val = json!({
                "type": "tool_start",
                "id": id,
                "tool": tool,
                "summary": summary,
            });
            val["kind"] = tool_kind_to_wire(kind);
            Some(val)
        }
        UiEvent::ToolOutput { id, event } => Some(json!({
            "type": "tool_output",
            "id": id,
            "event": tool_event_to_wire(event),
        })),
        UiEvent::ToolEnd {
            id,
            status,
            summary,
            model_content,
            result,
        } => {
            let mut val = json!({
                "type": "tool_end",
                "id": id,
                "status": status,
                "summary": summary,
            });
            if let Some(mc) = model_content {
                val["model_content"] = json!(mc);
            }
            if let Some(r) = result {
                val["result"] = r.clone();
            }
            Some(val)
        }
        UiEvent::PermissionRequested { request } => Some(json!({
            "type": "permission",
            "id": request.id,
            "tool": request.tool,
            "risk": request.risk,
            "summary": request.summary,
        })),
        UiEvent::Done {
            output,
            usage,
            turn,
        } => Some(json!({
            "type": "done",
            "session_id": output.session_id,
            "text": output.text,
            "turn": turn,
            "usage": usage,
        })),
        UiEvent::TurnStart { turn } => Some(json!({
            "type": "turn_start",
            "turn": turn,
        })),
        UiEvent::Cancelled { turn } => Some(json!({
            "type": "cancelled",
            "turn": turn,
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
    }
}

fn tool_kind_to_wire(kind: &crate::query::ToolKind) -> serde_json::Value {
    use crate::query::ToolKind;
    match kind {
        ToolKind::Simple => json!("simple"),
        ToolKind::Agent { child_session_id } => {
            json!({"agent": {"child_session_id": child_session_id}})
        }
        ToolKind::Command { pid } => json!({"command": {"pid": pid}}),
    }
}

fn tool_event_to_wire(event: &crate::query::ToolEvent) -> serde_json::Value {
    use crate::query::ToolEvent;
    match event {
        ToolEvent::TextDelta { text } => json!({"text": text}),
        ToolEvent::ThinkingDelta { text } => json!({"thinking": text}),
        ToolEvent::ToolStart {
            id,
            tool,
            summary,
            kind,
        } => {
            json!({"tool_start": {"id": id, "tool": tool, "summary": summary, "kind": tool_kind_to_wire(kind)}})
        }
        ToolEvent::ToolOutput { id, event } => {
            json!({"tool_output": {"id": id, "event": tool_event_to_wire(event)}})
        }
        ToolEvent::ToolEnd {
            id,
            status,
            summary,
        } => {
            json!({"tool_end": {"id": id, "status": status, "summary": summary}})
        }
        ToolEvent::Stdout { text } => json!({"stdout": text}),
        ToolEvent::Stderr { text } => json!({"stderr": text}),
        ToolEvent::PermissionRequested { request } => {
            json!({"permission": {"id": request.id, "tool": request.tool, "risk": request.risk, "summary": request.summary}})
        }
        ToolEvent::Error { code, message } => json!({"error": {"code": code, "message": message}}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{PermissionRequest, RunOutput, ToolEvent, ToolKind, UiEvent};

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
    fn tool_start_wire_all_kinds() {
        let simple = to_wire(&UiEvent::ToolStart {
            id: "t1".into(),
            tool: "find_files".into(),
            summary: "path: .".into(),
            kind: ToolKind::Simple,
        })
        .unwrap();
        assert_eq!(simple["kind"], "simple");
        assert_eq!(simple["type"], "tool_start");

        let cmd = to_wire(&UiEvent::ToolStart {
            id: "t2".into(),
            tool: "run_command".into(),
            summary: "cargo test".into(),
            kind: ToolKind::Command { pid: Some(42) },
        })
        .unwrap();
        assert_eq!(cmd["kind"]["command"]["pid"], 42);

        let agent = to_wire(&UiEvent::ToolStart {
            id: "t3".into(),
            tool: "agent".into(),
            summary: "code-review".into(),
            kind: ToolKind::Agent {
                child_session_id: "cs".into(),
            },
        })
        .unwrap();
        assert_eq!(agent["kind"]["agent"]["child_session_id"], "cs");
    }

    #[test]
    fn tool_output_wire_wraps_tool_event() {
        let event = UiEvent::ToolOutput {
            id: "t1".into(),
            event: ToolEvent::Stdout { text: "ok".into() },
        };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "tool_output");
        assert_eq!(wire["event"]["stdout"], "ok");
    }

    #[test]
    fn tool_end_wire_with_and_without_result() {
        let w1 = to_wire(&UiEvent::ToolEnd {
            id: "t1".into(),
            status: "ok".into(),
            summary: "done".into(),
            model_content: Some("content".into()),
            result: Some(serde_json::json!({"n": 1})),
        })
        .unwrap();
        assert_eq!(w1["model_content"], "content");
        assert_eq!(w1["result"]["n"], 1);

        let w2 = to_wire(&UiEvent::ToolEnd {
            id: "t2".into(),
            status: "cancelled".into(),
            summary: "".into(),
            model_content: None,
            result: None,
        })
        .unwrap();
        assert!(w2.get("model_content").is_none());
        assert!(w2.get("result").is_none());
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
        assert_eq!(wire["session_id"], "s1");
        assert_eq!(wire["text"], "done");
        assert_eq!(wire["turn"], 1);
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

    #[test]
    fn turn_start_wire_format_includes_turn() {
        let event = UiEvent::TurnStart { turn: 5 };
        let wire = to_wire(&event).unwrap();
        assert_eq!(wire["type"], "turn_start");
        assert_eq!(wire["turn"], 5);
    }
}
