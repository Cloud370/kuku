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

pub(crate) fn event_agent(payload: &kuku::event::EventPayload) -> Option<&str> {
    match payload {
        kuku::event::EventPayload::ModelResponse { conversation, .. }
        | kuku::event::EventPayload::ModelError { conversation, .. }
        | kuku::event::EventPayload::ToolCall { conversation, .. }
        | kuku::event::EventPayload::ToolResult { conversation, .. } => conversation.as_deref(),
        kuku::event::EventPayload::ConversationOpened { conversation, .. }
        | kuku::event::EventPayload::ConversationBound { conversation, .. }
        | kuku::event::EventPayload::PromptSnapshot { conversation, .. }
        | kuku::event::EventPayload::MessageUser { conversation, .. }
        | kuku::event::EventPayload::MessageAssistant { conversation, .. }
        | kuku::event::EventPayload::TurnStarted { conversation, .. }
        | kuku::event::EventPayload::TurnCompleted { conversation, .. }
        | kuku::event::EventPayload::TurnCancelled { conversation, .. }
        | kuku::event::EventPayload::TurnInterrupted { conversation, .. }
        | kuku::event::EventPayload::ConversationRollback { conversation, .. }
        | kuku::event::EventPayload::ConversationRollbackUndone { conversation, .. }
        | kuku::event::EventPayload::ModelRecovery { conversation, .. }
        | kuku::event::EventPayload::ContextSkills { conversation, .. } => Some(conversation),
        kuku::event::EventPayload::Unknown(value) => value
            .get("agent")
            .and_then(|item| item.as_str())
            .or_else(|| value.get("conversation").and_then(|item| item.as_str()))
            .or_else(|| {
                value
                    .get("record")
                    .and_then(|record| record.get("address"))
                    .and_then(|item| item.as_str())
            })
            .or_else(|| {
                value
                    .get("record")
                    .and_then(|record| record.get("input"))
                    .and_then(|input| input.get("target"))
                    .and_then(|item| item.as_str())
            }),
        _ => None,
    }
}

pub(crate) fn serialize_agent_event(event: &UiEvent, agent: &str) -> Option<String> {
    let line = serialize_event(event)?;
    let mut value: serde_json::Value = serde_json::from_str(&line).ok()?;
    value["agent"] = serde_json::Value::String(agent.into());
    Some(format!("{value}\n"))
}

/// Encode the durable input receipt and explicit HTTP run identity.
pub fn run_start(
    run_id: &str,
    session_id: &str,
    agent: &str,
    receipt: &kuku::session::Receipt,
) -> String {
    let value = serde_json::json!({
        "type": "run_start",
        "run_id": run_id,
        "session_id": session_id,
        "agent": agent,
        "receipt": receipt,
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
