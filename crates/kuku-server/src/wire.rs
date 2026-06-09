use kuku::query::UiEvent;

pub fn serialize_event(event: &UiEvent) -> Option<String> {
    let mut value = kuku::wire::to_wire(event)?;
    match event {
        UiEvent::ToolStart {
            kind: kuku::query::ToolKind::Agent { conversation, .. },
            ..
        } => {
            value["conversation"] = serde_json::Value::String(conversation.as_str().to_string());
        }
        UiEvent::PermissionRequested { request } => {
            value["conversation"] =
                serde_json::Value::String(request.conversation.as_str().to_string());
        }
        UiEvent::ToolOutput {
            event: kuku::query::ToolEvent::PermissionRequested { request },
            ..
        } => {
            value["conversation"] =
                serde_json::Value::String(request.conversation.as_str().to_string());
        }
        UiEvent::Done { output, .. } => {
            value["conversation"] =
                serde_json::Value::String(output.conversation.as_str().to_string());
        }
        _ => {}
    }
    let mut line = serde_json::to_string(&value).ok()?;
    line.push('\n');
    Some(line)
}

pub fn run_start(run_id: &str) -> String {
    let value = serde_json::json!({
        "type": "run_start",
        "run_id": run_id,
    });
    format!("{}\n", value)
}

#[cfg(test)]
mod tests {
    use kuku::conversation::address::ConversationAddress;
    use kuku::query::{PermissionRequest, ToolEvent, UiEvent};

    use super::serialize_event;

    #[test]
    fn nested_permission_output_includes_top_level_conversation() {
        let event = UiEvent::ToolOutput {
            id: "parent_tool".to_string(),
            event: ToolEvent::PermissionRequested {
                request: PermissionRequest {
                    id: "req_1".to_string(),
                    conversation: ConversationAddress::parse("review").unwrap(),
                    turn: 1,
                    tool_call_id: "child_tool".to_string(),
                    tool: "run_command".to_string(),
                    risk: "command".to_string(),
                    summary: "cargo test".to_string(),
                    candidate: "cargo test".to_string(),
                    source: "default_ask".to_string(),
                },
            },
        };

        let line = serialize_event(&event).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();

        assert_eq!(value["conversation"], "review");
        assert_eq!(value["type"], "tool_output");
        assert_eq!(value["event"]["permission"]["id"], "req_1");
    }
}
