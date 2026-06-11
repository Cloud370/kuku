use super::*;

#[test]
fn handoff_round_trip() {
    let event = StoredEvent {
        id: 43,
        payload: EventPayload::Handoff {
            turn: 3,
            ts: "2026-05-27T00:00:01Z".to_string(),
            request_id: "req_3".to_string(),
            summary: "## Goal\nBuild feature X".to_string(),
            keep_turns: 2,
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: StoredEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn handoff_event_type_tag_is_handoff() {
    let event = StoredEvent {
        id: 1,
        payload: EventPayload::Handoff {
            turn: 1,
            ts: "t".to_string(),
            request_id: "req_1".to_string(),
            summary: "s".to_string(),
            keep_turns: 0,
        },
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "handoff");
}

#[test]
fn rollback_scope_variants_serialize_correctly() {
    let cases = [
        (RollbackScope::ConversationOnly, r#""messages""#),
        (RollbackScope::FilesOnly, r#""file_changes""#),
        (RollbackScope::Both, r#""both""#),
    ];
    for (variant, expected) in &cases {
        assert_eq!(serde_json::to_string(variant).unwrap(), *expected);
        let back: RollbackScope = serde_json::from_str(expected).unwrap();
        assert_eq!(back, *variant);
    }
}

#[test]
fn conversation_rollback_round_trip() {
    let event = StoredEvent {
        id: 51,
        payload: EventPayload::ConversationRollback {
            ts: "2026-05-28T00:01:00Z".to_string(),
            conversation: "main".to_string(),
            to_turn: 3,
            to_event_id: 9,
            scope: RollbackScope::Both,
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: StoredEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn conversation_rollback_undo_round_trip() {
    let event = StoredEvent {
        id: 1,
        payload: EventPayload::ConversationRollbackUndone {
            ts: "t".to_string(),
            conversation: "main".to_string(),
            rollback_event_id: 9,
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: StoredEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn conversation_rollback_event_type_tag() {
    let event = StoredEvent {
        id: 1,
        payload: EventPayload::ConversationRollback {
            ts: "t".to_string(),
            conversation: "main".to_string(),
            to_turn: 1,
            to_event_id: 3,
            scope: RollbackScope::ConversationOnly,
        },
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap()["kind"],
        "conversation.rollback"
    );
}
