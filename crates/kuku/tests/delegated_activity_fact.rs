use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, ConversationId, RunId, TaskActivityBatch,
    TaskEvent,
};

fn run_id() -> RunId {
    RunId::parse("run_0123456789abcdef01234567").unwrap()
}

fn conversation_id() -> ConversationId {
    ConversationId::parse("con_0123456789abcdef01234567").unwrap()
}

fn schema_contains_null(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(value) => value == "null",
        serde_json::Value::Array(values) => values.iter().any(schema_contains_null),
        serde_json::Value::Object(values) => values.values().any(schema_contains_null),
        _ => false,
    }
}

#[test]
fn delegated_activity_round_trips_complete_typed_identity() {
    let activity = ActivityFact {
        activity_id: "act_delegate_review".to_owned(),
        run_id: run_id(),
        title: "Review changes".to_owned(),
        kind: ActivityKindFact::DelegatedAgent,
        status: ActivityStatusFact::Completed,
        detail: None,
        conversation_id: Some(conversation_id()),
        agent: Some("agent:project:review".to_owned()),
        tier: Some("tier:balanced".to_owned()),
        result_in_main: Some(true),
        file_references: vec![],
    };

    let expected = serde_json::json!({
        "activity_id": "act_delegate_review",
        "run_id": "run_0123456789abcdef01234567",
        "title": "Review changes",
        "kind": "delegated_agent",
        "status": "completed",
        "detail": null,
        "conversation_id": "con_0123456789abcdef01234567",
        "agent": "agent:project:review",
        "tier": "tier:balanced",
        "result_in_main": true,
        "file_references": [],
    });
    let encoded = serde_json::to_value(&activity).unwrap();
    assert_eq!(encoded, expected);

    let decoded: ActivityFact = serde_json::from_value(expected).unwrap();
    assert_eq!(decoded, activity);
}

#[test]
fn non_delegated_activity_serializes_typed_identity_as_null() {
    let activity = ActivityFact {
        activity_id: "act_tool_read".to_owned(),
        run_id: run_id(),
        title: "Read file".to_owned(),
        kind: ActivityKindFact::Tool,
        status: ActivityStatusFact::Running,
        detail: None,
        conversation_id: None,
        agent: None,
        tier: None,
        result_in_main: None,
        file_references: vec![],
    };

    let encoded = serde_json::to_value(&activity).unwrap();
    assert_eq!(encoded["conversation_id"], serde_json::Value::Null);
    assert_eq!(encoded["agent"], serde_json::Value::Null);
    assert_eq!(encoded["tier"], serde_json::Value::Null);
    assert_eq!(encoded["result_in_main"], serde_json::Value::Null);

    let decoded: ActivityFact = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, activity);
}

#[test]
fn delegated_identity_fields_are_required_even_when_null() {
    let missing_conversation_id = serde_json::json!({
        "activity_id": "act_tool_read",
        "run_id": "run_0123456789abcdef01234567",
        "title": "Read file",
        "kind": "tool",
        "status": "running",
        "detail": null,
        "agent": null,
        "tier": null,
        "result_in_main": null,
        "file_references": [],
    });

    assert!(serde_json::from_value::<ActivityFact>(missing_conversation_id).is_err());

    let schema = serde_json::to_value(schemars::schema_for!(ActivityFact)).unwrap();
    let required = schema["required"].as_array().unwrap();
    for field in ["conversation_id", "agent", "tier", "result_in_main"] {
        assert!(required.contains(&serde_json::Value::String(field.to_owned())));
        assert!(schema_contains_null(&schema["properties"][field]));
    }
}

#[test]
fn ledger_rejects_missing_or_misplaced_delegated_identity() {
    let missing_identity = ActivityFact {
        activity_id: "act_delegate_review".to_owned(),
        run_id: run_id(),
        title: "Review changes".to_owned(),
        kind: ActivityKindFact::DelegatedAgent,
        status: ActivityStatusFact::Running,
        detail: None,
        conversation_id: None,
        agent: None,
        tier: None,
        result_in_main: None,
        file_references: vec![],
    };
    assert!(
        TaskActivityBatch::try_new(vec![TaskEvent::ActivityUpserted {
            activity: missing_identity,
        }])
        .is_err()
    );

    let misplaced_identity = ActivityFact {
        activity_id: "act_tool_read".to_owned(),
        run_id: run_id(),
        title: "Read file".to_owned(),
        kind: ActivityKindFact::Tool,
        status: ActivityStatusFact::Completed,
        detail: None,
        conversation_id: Some(conversation_id()),
        agent: Some("agent:project:review".to_owned()),
        tier: Some("tier:balanced".to_owned()),
        result_in_main: Some(false),
        file_references: vec![],
    };
    assert!(
        TaskActivityBatch::try_new(vec![TaskEvent::ActivityUpserted {
            activity: misplaced_identity,
        }])
        .is_err()
    );

    let delegated_detail = ActivityFact {
        activity_id: "act_delegate_review".to_owned(),
        run_id: run_id(),
        title: "Review changes".to_owned(),
        kind: ActivityKindFact::DelegatedAgent,
        status: ActivityStatusFact::Completed,
        detail: Some("agent=review tier=balanced".to_owned()),
        conversation_id: Some(conversation_id()),
        agent: Some("agent:project:review".to_owned()),
        tier: Some("tier:balanced".to_owned()),
        result_in_main: Some(true),
        file_references: vec![],
    };
    assert!(
        TaskActivityBatch::try_new(vec![TaskEvent::ActivityUpserted {
            activity: delegated_detail,
        }])
        .is_err()
    );
}
