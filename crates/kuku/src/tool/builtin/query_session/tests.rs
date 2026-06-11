use super::*;
use crate::event::{EventPayload, EventStore};
use serde_json::json;
use tempfile::tempdir;

fn write_events(dir: &Path, payloads: &[EventPayload]) -> std::path::PathBuf {
    let path = dir.join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    for payload in payloads {
        store.append(payload.clone()).unwrap();
    }
    path
}

fn ts(s: &str) -> String {
    s.to_string()
}

fn message_user(turn: u64, text: &str) -> EventPayload {
    EventPayload::MessageUser {
        turn,
        ts: ts("t"),
        conversation: "main".into(),
        text: text.into(),
        from: None,
        via_tool_call_id: None,
    }
}

fn conversation_rollback(to_turn: u64, to_event_id: u64) -> EventPayload {
    EventPayload::ConversationRollback {
        ts: ts("t"),
        conversation: "main".into(),
        to_turn,
        to_event_id,
        scope: crate::event::RollbackScope::ConversationOnly,
    }
}

#[test]
fn query_session_filters_by_kind() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            message_user(1, "hello"),
            EventPayload::ModelResponse {
                turn: 1,
                ts: ts("t"),
                request_id: "r1".into(),
                text: "hi".into(),
                thinking: None,
                input_tokens_total: None,
            },
            EventPayload::TurnCompleted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
        ],
    );
    let result = query_session(&json!({"kind": "message.user"}), &path);
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("hello"));
    assert!(!result.model_content.contains("\"hi\""));
}

#[test]
fn query_session_rejects_non_canonical_kind_aliases() {
    let dir = tempdir().unwrap();
    let path = write_events(dir.path(), &[message_user(1, "hello")]);

    let result = query_session(&json!({"kind": "MessageUser"}), &path);

    assert_eq!(result.status, "ok");
    assert_eq!(result.model_content, "[\n\n]");
}

#[test]
fn query_session_default_results_exclude_context_skills() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            EventPayload::ContextSkills {
                conversation: "main".into(),
                turn: 1,
                ts: ts("t"),
                registry: serde_json::to_value(
                    crate::skill::registry::SkillRegistry::builder().build(),
                )
                .unwrap(),
                bootstrap_loaded: vec!["bootstrap-alpha".into()],
            },
            message_user(1, "hello"),
        ],
    );

    let default_result = query_session(&json!({}), &path);
    assert_eq!(default_result.status, "ok");
    assert!(default_result.model_content.contains("hello"));
    assert!(!default_result.model_content.contains("context.skills"));
    assert!(!default_result.model_content.contains("bootstrap-alpha"));

    let explicit_result = query_session(&json!({"kind": "context.skills"}), &path);
    assert_eq!(explicit_result.status, "ok");
    assert!(explicit_result.model_content.contains("context.skills"));
    assert!(explicit_result.model_content.contains("bootstrap-alpha"));
}

#[test]
fn query_session_text_search() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[message_user(1, "build auth"), message_user(1, "fix bug")],
    );
    let result = query_session(&json!({"search": "auth"}), &path);
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("build auth"));
    assert!(!result.model_content.contains("fix bug"));
}

#[test]
fn query_session_respects_limit() {
    let dir = tempdir().unwrap();
    let mut payloads = Vec::new();
    for i in 0..30 {
        payloads.push(message_user(1, &format!("msg {i}")));
    }
    let path = write_events(dir.path(), &payloads);
    let result = query_session(&json!({"limit": 5}), &path);
    assert_eq!(result.status, "ok");
    let count = result.model_content.matches("\"id\":").count();
    assert_eq!(count, 5);
}

#[test]
fn query_session_turn_filtering() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            EventPayload::MessageUser {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
                text: "first turn".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
                text: "second turn".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
            },
        ],
    );
    let result = query_session(&json!({"from_turn": 0, "to_turn": 0}), &path);
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("second turn"));
    assert!(!result.model_content.contains("first turn"));
}

#[test]
fn query_session_empty_events() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let result = query_session(&json!({}), &path);
    assert_eq!(result.status, "ok");
    assert_eq!(result.model_content, "[]");
}

#[test]
fn query_session_truncates_long_content() {
    let dir = tempdir().unwrap();
    let long_text = "x".repeat(1000);
    let path = write_events(dir.path(), &[message_user(1, &long_text)]);
    let result = query_session(&json!({}), &path);
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("...(truncated)"));
    let full = "x".repeat(1000);
    assert!(!result.model_content.contains(&full));
}

#[test]
fn query_session_output_cap_drops_earliest_events() {
    let dir = tempdir().unwrap();
    let big_text = "y".repeat(3000);
    let mut payloads = Vec::new();
    for i in 0..5 {
        payloads.push(message_user(1, &format!("msg_{i}_{big_text}")));
    }
    let path = write_events(dir.path(), &payloads);
    let result = query_session(&json!({}), &path);
    assert_eq!(result.status, "ok");
    assert!(
        result.model_content.len() < MAX_RESULT_CHARS + 2000,
        "output too large: {} chars",
        result.model_content.len()
    );
}

#[test]
fn query_session_skip_rolled_back_filters_events() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            EventPayload::TurnStarted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
                text: "original".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::TurnStarted {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
                text: "rolled back".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
            },
            conversation_rollback(2, 6),
        ],
    );
    let result = query_session(&json!({"skip_rolled_back": true}), &path);
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("original"));
    assert!(!result.model_content.contains("rolled back"));
}

#[test]
fn query_session_skip_rolled_back_false_returns_all() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            EventPayload::TurnStarted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
                text: "original".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::TurnStarted {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
                text: "rolled back".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
            },
            conversation_rollback(2, 6),
        ],
    );
    let result = query_session(&json!({"skip_rolled_back": false}), &path);
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("original"));
    assert!(result.model_content.contains("rolled back"));
}

#[test]
fn query_session_kind_filter_conversation_rollback() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            message_user(1, "hello"),
            EventPayload::ConversationRollback {
                ts: ts("t"),
                conversation: "main".into(),
                to_turn: 1,
                to_event_id: 1,
                scope: crate::event::RollbackScope::Both,
            },
        ],
    );
    let result = query_session(
        &json!({"kind": "conversation.rollback", "skip_rolled_back": false}),
        &path,
    );
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("conversation.rollback"));
    assert!(!result.model_content.contains("hello"));
}

#[test]
fn query_session_kind_filter_permission_requested() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            message_user(1, "hello"),
            EventPayload::PermissionRequested {
                turn: 1,
                ts: ts("t"),
                tool_call_id: "toolu_cmd".into(),
                tool: "run_command".into(),
                risk: "command".into(),
                summary: "run tests".into(),
                candidate: "cargo test".into(),
                source: "default_ask".into(),
            },
        ],
    );

    let result = query_session(&json!({"kind": "permission.requested"}), &path);

    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("permission.requested"));
    assert!(result.model_content.contains("cargo test"));
    assert!(!result.model_content.contains("hello"));
}

#[test]
fn query_session_type_filter_permission_allow_and_deny() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            message_user(1, "hello"),
            EventPayload::PermissionAllow {
                turn: 1,
                ts: ts("t"),
                tool_call_id: "toolu_allow".into(),
                tool: "run_command".into(),
                scope: "session".into(),
                matcher: "cargo test".into(),
                source: "user".into(),
            },
            EventPayload::PermissionDeny {
                turn: 1,
                ts: ts("t"),
                tool_call_id: "toolu_deny".into(),
                tool: "run_command".into(),
                reason: "too risky".into(),
                source: "user".into(),
            },
        ],
    );

    let allow = query_session(&json!({"kind": "permission.allow"}), &path);
    let deny = query_session(&json!({"kind": "permission.deny"}), &path);

    assert_eq!(allow.status, "ok");
    assert!(allow.model_content.contains("permission.allow"));
    assert!(allow.model_content.contains("cargo test"));
    assert!(!allow.model_content.contains("permission.deny"));
    assert!(!allow.model_content.contains("hello"));
    assert_eq!(deny.status, "ok");
    assert!(deny.model_content.contains("permission.deny"));
    assert!(deny.model_content.contains("too risky"));
    assert!(!deny.model_content.contains("permission.allow"));
    assert!(!deny.model_content.contains("hello"));
}

#[test]
fn query_session_turn_filtering_respects_skip_rolled_back_stream() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            EventPayload::TurnStarted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
                text: "first".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::TurnStarted {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
                text: "second active".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 2,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::TurnStarted {
                turn: 3,
                ts: ts("t"),
                conversation: "main".into(),
            },
            EventPayload::MessageUser {
                turn: 3,
                ts: ts("t"),
                conversation: "main".into(),
                text: "third rolled back".into(),
                from: None,
                via_tool_call_id: None,
            },
            EventPayload::TurnCompleted {
                turn: 3,
                ts: ts("t"),
                conversation: "main".into(),
            },
            conversation_rollback(3, 9),
        ],
    );

    let result = query_session(
        &json!({"skip_rolled_back": true, "from_turn": 1, "to_turn": 1}),
        &path,
    );

    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("first"));
    assert!(!result.model_content.contains("second active"));
    assert!(!result.model_content.contains("third rolled back"));
}

#[test]
fn query_session_filters_single_conversation() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            EventPayload::ConversationOpened {
                ts: ts("t"),
                conversation: "review".into(),
            },
            EventPayload::MessageUser {
                ts: ts("t"),
                conversation: "review".into(),
                turn: 1,
                text: "review text".into(),
                from: Some("main".into()),
                via_tool_call_id: None,
            },
            EventPayload::ConversationOpened {
                ts: ts("t"),
                conversation: "explore".into(),
            },
            EventPayload::MessageUser {
                ts: ts("t"),
                conversation: "explore".into(),
                turn: 1,
                text: "explore text".into(),
                from: Some("main".into()),
                via_tool_call_id: None,
            },
        ],
    );

    let result = query_session(&json!({"conversation": "review"}), &path);
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("message.user"));
    assert!(result.model_content.contains("review"));
    assert!(result.model_content.contains("review text"));
    assert!(!result.model_content.contains("explore text"));
}

#[test]
fn query_session_main_conversation_includes_main_model_response() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[
            message_user(1, "hello"),
            EventPayload::ModelResponse {
                turn: 1,
                ts: ts("t"),
                request_id: "r1".into(),
                text: "main answer".into(),
                thinking: None,
                input_tokens_total: None,
            },
            EventPayload::TurnCompleted {
                turn: 1,
                ts: ts("t"),
                conversation: "main".into(),
            },
        ],
    );

    let result = query_session(&json!({"conversation": "main"}), &path);

    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("hello"));
    assert!(result.model_content.contains("main answer"));
}

#[test]
fn query_session_filters_after_event_id() {
    let dir = tempdir().unwrap();
    let path = write_events(
        dir.path(),
        &[message_user(1, "first"), message_user(1, "second")],
    );

    let result = query_session(&json!({"after": 1}), &path);
    assert_eq!(result.status, "ok");
    assert!(!result.model_content.contains("first"));
    assert!(result.model_content.contains("second"));
}
