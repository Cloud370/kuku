use super::{effective_snapshot_events, SUPPLIED_FULL_WRITE};
use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, ModelStopReason, RollbackScope, StoredEvent};

fn read_event(id: u64, turn: u64, conversation: Option<&str>) -> StoredEvent {
    snapshot_event(
        id,
        turn,
        conversation,
        serde_json::json!({"kind": "file_content"}),
    )
}

fn snapshot_event(
    id: u64,
    turn: u64,
    conversation: Option<&str>,
    facts: serde_json::Value,
) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ToolResult {
            turn,
            ts: "ts".to_string(),
            conversation: conversation.map(str::to_string),
            tool_call_id: format!("read_{id}"),
            status: "ok".to_string(),
            summary: "read".to_string(),
            model_content: "1\tvisible".to_string(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: Some(facts),
        },
    }
}

#[test]
fn legacy_full_write_requires_a_later_response_in_the_effective_conversation() {
    let response = |id, agent: &str| StoredEvent {
        id,
        payload: EventPayload::ModelResponse {
            conversation: Some(agent.into()),
            turn: 1,
            ts: String::new(),
            request_id: format!("req_{id}"),
            text: String::new(),
            thinking: None,
            stop_reason: Some(ModelStopReason::ToolUse),
            input_tokens_total: None,
            output_tokens_total: None,
        },
    };
    let write = snapshot_event(
        2,
        1,
        Some("main"),
        serde_json::json!({
            "kind": "file_write", "canonical_path": "fixture.txt", "commit_state": "committed",
            "content_hash_after": "digest", (SUPPLIED_FULL_WRITE): true,
        }),
    );
    let snapshots = |events: &[StoredEvent]| {
        ids(effective_snapshot_events(
            events,
            &ConversationAddress::MAIN,
        ))
    };
    let mut events = vec![response(1, "main"), write, response(3, "reviewer")];
    assert!(snapshots(&events).is_empty());

    events.push(response(4, "main"));
    assert_eq!(vec![2], snapshots(&events));
    events.push(rollback_for(5, "main", 1, 3));
    assert!(snapshots(&events).is_empty());

    events.pop();
    events.push(handoff(5, 1, 0));
    assert!(snapshots(&events).is_empty());
}

fn rollback_for(id: u64, conversation: &str, to_turn: u64, to_event_id: u64) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ConversationRollback {
            ts: "ts".to_string(),
            conversation: conversation.to_string(),
            to_turn,
            to_event_id,
            scope: RollbackScope::ConversationOnly,
        },
    }
}

fn handoff(id: u64, turn: u64, keep_turns: usize) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::Handoff {
            turn,
            ts: "ts".to_string(),
            request_id: format!("request_{id}"),
            summary: "continue".to_string(),
            keep_turns,
        },
    }
}

fn ids(events: Vec<&StoredEvent>) -> Vec<u64> {
    events.into_iter().map(|event| event.id).collect()
}

#[test]
fn effective_snapshot_events_are_scoped_to_active_conversation() {
    let events = vec![
        read_event(1, 1, Some("review")),
        read_event(2, 1, None),
        read_event(3, 1, Some("main")),
    ];

    assert_eq!(
        ids(effective_snapshot_events(
            &events,
            &ConversationAddress::MAIN
        )),
        vec![2, 3]
    );
    assert_eq!(
        ids(effective_snapshot_events(
            &events,
            &ConversationAddress::parse("review").unwrap()
        )),
        vec![1]
    );
}

#[test]
fn effective_snapshot_events_exclude_active_rollback_tail() {
    let events = vec![
        read_event(1, 1, None),
        read_event(2, 2, None),
        rollback_for(3, "main", 2, 1),
    ];

    assert_eq!(
        ids(effective_snapshot_events(
            &events,
            &ConversationAddress::MAIN
        )),
        vec![1]
    );
}

#[test]
fn effective_snapshot_events_exclude_delegated_rollback_tail() {
    let events = vec![
        read_event(1, 1, Some("review")),
        read_event(2, 2, Some("review")),
        rollback_for(3, "review", 2, 1),
    ];

    assert_eq!(
        ids(effective_snapshot_events(
            &events,
            &ConversationAddress::parse("review").unwrap()
        )),
        vec![1]
    );
}

#[test]
fn effective_snapshot_events_exclude_compacted_handoff_history() {
    let events = vec![
        read_event(1, 1, None),
        read_event(2, 2, None),
        read_event(3, 3, None),
        handoff(4, 3, 1),
    ];

    assert_eq!(
        ids(effective_snapshot_events(
            &events,
            &ConversationAddress::MAIN
        )),
        vec![3]
    );
}

#[test]
fn effective_snapshot_events_accept_visible_read_after_handoff() {
    let events = vec![
        read_event(1, 1, None),
        handoff(2, 1, 0),
        read_event(3, 2, None),
    ];

    assert_eq!(
        ids(effective_snapshot_events(
            &events,
            &ConversationAddress::MAIN
        )),
        vec![3]
    );
}
