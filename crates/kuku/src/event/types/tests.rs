use super::*;

#[test]
fn model_response_round_trips_scoped_terminal_state_and_u64_usage() {
    let event = StoredEvent {
        id: 1,
        payload: EventPayload::ModelResponse {
            conversation: Some("delegate".to_string()),
            turn: 7,
            ts: "t".to_string(),
            request_id: "req_1".to_string(),
            text: "partial".to_string(),
            thinking: None,
            stop_reason: Some(ModelStopReason::Length),
            input_tokens_total: Some(u64::from(u32::MAX) + 1),
            output_tokens_total: Some(0),
        },
    };

    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(Some("delegate"), json["conversation"].as_str());
    assert_eq!(Some("length"), json["stop_reason"].as_str());
    assert_eq!(Some(u64::from(u32::MAX) + 1), json["input_tokens_total"].as_u64());
    assert_eq!(Some(0), json["output_tokens_total"].as_u64());
    assert_eq!(event, serde_json::from_value(json).unwrap());
}

#[test]
fn legacy_model_response_defaults_to_main_and_preserves_unknown_stop_reason() {
    let event: StoredEvent = serde_json::from_value(serde_json::json!({
        "id": 1,
        "kind": "model.response",
        "turn": 7,
        "ts": "t",
        "request_id": "req_1",
        "text": "partial",
        "stop_reason": "provider_limit",
    }))
    .unwrap();

    assert!(matches!(
        event.payload,
        EventPayload::ModelResponse {
            conversation: None,
            stop_reason: Some(ModelStopReason::Unknown(ref reason)),
            ..
        } if reason == "provider_limit"
    ));
}

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
