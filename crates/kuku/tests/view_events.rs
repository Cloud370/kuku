use kuku::event::{EventPayload, StoredEvent};
use kuku::view::format_event_brief;

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
    let line = format_event_brief(&event, false);
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
    let line = format_event_brief(&event, true);
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
            stop_reason: "end_turn".into(),
            tool_call_count: None,
            usage: serde_json::json!({}),
        },
    );
    let line = format_event_brief(&event, false);
    assert!(line.contains("model.response"), "should contain type");
    assert!(line.contains("Hello"), "should contain text");
}
