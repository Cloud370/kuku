use kuku::event::{EventPayload, StoredEvent};

pub fn render_event_brief(event: &StoredEvent, verbose: bool) -> String {
    let mut line = format!("evt:{} | {}", event.id, event_type_name(&event.payload));
    let details = event_details(&event.payload, verbose);
    if !details.is_empty() {
        line.push_str(" | ");
        line.push_str(&details);
    }
    line
}

fn event_type_name(payload: &EventPayload) -> &'static str {
    match payload {
        EventPayload::SessionMeta { .. } => "session.meta",
        EventPayload::TurnStart { .. } => "turn.start",
        EventPayload::UserInput { .. } => "user.input",
        EventPayload::ModelRequest { .. } => "model.request",
        EventPayload::ModelResponse { .. } => "model.response",
        EventPayload::ModelError { .. } => "model.error",
        EventPayload::ToolCall { .. } => "tool.call",
        EventPayload::PermissionRequest { .. } => "permission.request",
        EventPayload::PermissionDecision { .. } => "permission.decision",
        EventPayload::ToolResult { .. } => "tool.result",
        EventPayload::PolicyLoaded { .. } => "policy.loaded",
        EventPayload::TurnEnd { .. } => "turn.end",
    }
}

fn event_details(payload: &EventPayload, verbose: bool) -> String {
    match payload {
        EventPayload::UserInput { text, .. } => text.chars().take(60).collect(),
        EventPayload::ModelResponse {
            text, stop_reason, ..
        } => {
            let preview: String = text.chars().take(60).collect();
            format!("{preview}  stop={stop_reason}")
        }
        EventPayload::ToolCall {
            tool,
            args,
            tool_call_id,
            ..
        } => {
            let path_or_cmd = args
                .get("path")
                .or_else(|| args.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if verbose {
                format!("{tool} {path_or_cmd}  id={tool_call_id}")
            } else {
                format!("{tool} {path_or_cmd}")
            }
        }
        EventPayload::ToolResult {
            tool_call_id,
            status,
            summary,
            ..
        } => {
            if verbose {
                format!("{status}  {summary}  id={tool_call_id}")
            } else {
                format!("{status}  {summary}")
            }
        }
        EventPayload::PermissionDecision { decision, rule, .. } => {
            format!("{decision}  {rule}")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(id: u64, payload: EventPayload) -> StoredEvent {
        StoredEvent { id, payload }
    }

    #[test]
    fn formats_tool_call_concisely() {
        let event = stored(
            5,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-01-01T00:00:00Z".into(),
                tool_call_id: "tc_1".into(),
                request_id: "req_1".into(),
                index: 0,
                tool: "read_file".into(),
                args: serde_json::json!({"path": "README.md"}),
            },
        );
        let line = render_event_brief(&event, false);
        assert!(line.contains("evt:5"), "should contain evt:5, got: {line}");
        assert!(
            line.contains("tool.call"),
            "should contain type, got: {line}"
        );
        assert!(
            line.contains("read_file"),
            "should contain tool name, got: {line}"
        );
    }

    #[test]
    fn formats_tool_call_verbose() {
        let event = stored(
            5,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-01-01T00:00:00Z".into(),
                tool_call_id: "tc_readme1".into(),
                request_id: "req_1".into(),
                index: 0,
                tool: "read_file".into(),
                args: serde_json::json!({"path": "README.md"}),
            },
        );
        let line = render_event_brief(&event, true);
        assert!(
            line.contains("tc_readme1"),
            "verbose should contain tool_call_id, got: {line}"
        );
    }

    #[test]
    fn formats_model_response() {
        let event = stored(
            3,
            EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-01-01T00:00:00Z".into(),
                request_id: "req_1".into(),
                text: "Hello".into(),
                thinking: None,
                stop_reason: "end_turn".into(),
                tool_call_count: None,
                usage: serde_json::json!({}),
            },
        );
        let line = render_event_brief(&event, false);
        assert!(line.contains("model.response"), "should contain type");
        assert!(line.contains("Hello"), "should contain text");
    }
}
