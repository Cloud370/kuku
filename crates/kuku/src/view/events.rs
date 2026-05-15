use crate::event::{EventPayload, StoredEvent};

pub fn format_event_brief(event: &StoredEvent, verbose: bool) -> String {
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
