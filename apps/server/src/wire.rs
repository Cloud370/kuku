use kuku::query::UiEvent;

pub fn serialize_event(event: &UiEvent) -> Option<String> {
    let value = kuku::wire::to_wire(event)?;
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
