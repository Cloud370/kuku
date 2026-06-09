use kuku::query::UiEvent;

pub fn serialize_event(event: &UiEvent) -> Option<String> {
    let mut value = kuku::wire::to_wire(event)?;
    match event {
        UiEvent::ToolStart {
            kind: kuku::query::ToolKind::Agent { conversation, .. },
            ..
        } => {
            value["conversation"] =
                serde_json::Value::String(conversation.as_str().to_string());
        }
        UiEvent::PermissionRequested { request } => {
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
