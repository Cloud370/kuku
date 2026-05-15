use kuku::event::{EventPayload, StoredEvent};
use kuku::view::derive_final_output;

fn stored(id: u64, payload: EventPayload) -> StoredEvent {
    StoredEvent { id, payload }
}

#[test]
fn finds_last_end_turn_response() {
    let events = vec![
        stored(
            1,
            EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-01-01T00:00:00Z".into(),
                request_id: "req_1".into(),
                text: "first".into(),
                thinking: None,
                stop_reason: "tool_use".into(),
                tool_call_count: Some(1),
                usage: serde_json::json!({}),
            },
        ),
        stored(
            2,
            EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-01-01T00:00:01Z".into(),
                request_id: "req_2".into(),
                text: "final answer".into(),
                thinking: None,
                stop_reason: "end_turn".into(),
                tool_call_count: None,
                usage: serde_json::json!({}),
            },
        ),
    ];
    assert_eq!(derive_final_output(&events), Some("final answer".into()));
}

#[test]
fn returns_none_when_no_end_turn() {
    let events = vec![stored(
        1,
        EventPayload::ModelResponse {
            turn: 1,
            ts: "2026-01-01T00:00:00Z".into(),
            request_id: "req_1".into(),
            text: "not done".into(),
            thinking: None,
            stop_reason: "tool_use".into(),
            tool_call_count: Some(1),
            usage: serde_json::json!({}),
        },
    )];
    assert_eq!(derive_final_output(&events), None);
}

#[test]
fn returns_none_for_empty_events() {
    assert_eq!(derive_final_output(&[]), None);
}
