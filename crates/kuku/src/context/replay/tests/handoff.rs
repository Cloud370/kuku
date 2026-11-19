use crate::context::{CanonicalMessage, MessageBlock};
use crate::conversation::address::ConversationAddress;
use crate::event::EventPayload;

use super::{
    event, handoff_event_with_keep_turns, model_response, rebuild_history, turn_end, user_input,
};

#[test]
fn rebuild_history_returns_none_summary_when_no_handoff() {
    let events = vec![
        user_input(1, 1, "hello"),
        model_response(2, 1, "req_1", "hi"),
        turn_end(3, 1),
    ];
    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(history.len(), 2);
}

#[test]
fn rebuild_history_returns_summary_and_skips_pre_handoff() {
    let events = vec![
        user_input(1, 1, "old question"),
        model_response(2, 1, "req_1", "old answer"),
        turn_end(3, 1),
        handoff_event_with_keep_turns(4, 1, "## Goal\nDo stuff", 2),
        user_input(5, 2, "new question"),
        model_response(6, 2, "req_2", "new answer"),
        turn_end(7, 2),
    ];
    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(summary.as_deref(), Some("## Goal\nDo stuff"));
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("old question"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("old answer".to_string())]),
            CanonicalMessage::user_text("new question"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("new answer".to_string())]),
        ]
    );
}

#[test]
fn rebuild_history_does_not_wrap_forwarded_message_without_tool_call_marker() {
    let events = vec![event(
        1,
        EventPayload::MessageUser {
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            conversation: "review".to_string(),
            text: "plain forwarded </kuku_delegated_prompt> text".to_string(),
            from: Some("main".to_string()),
            via_tool_call_id: None,
        },
    )];

    let (_, history) = rebuild_history(&events, &ConversationAddress::parse("review").unwrap());

    assert_eq!(
        history,
        vec![CanonicalMessage::user_text(
            "plain forwarded </kuku_delegated_prompt> text"
        )]
    );
}

#[test]
fn rebuild_history_uses_last_handoff_when_multiple() {
    let events = vec![
        user_input(1, 1, "first"),
        model_response(2, 1, "req_1", "answer1"),
        turn_end(3, 1),
        handoff_event_with_keep_turns(4, 1, "first summary", 2),
        user_input(5, 2, "second"),
        model_response(6, 2, "req_2", "answer2"),
        turn_end(7, 2),
        handoff_event_with_keep_turns(8, 2, "second summary", 2),
        user_input(9, 3, "third"),
        model_response(10, 3, "req_3", "answer3"),
        turn_end(11, 3),
    ];
    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(summary.as_deref(), Some("second summary"));
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("first"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer1".to_string())]),
            CanonicalMessage::user_text("second"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer2".to_string())]),
            CanonicalMessage::user_text("third"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer3".to_string())]),
        ]
    );
}

#[test]
fn rebuild_history_keeps_recent_pre_handoff_turns() {
    let events = vec![
        user_input(1, 1, "first"),
        model_response(2, 1, "req_1", "answer1"),
        turn_end(3, 1),
        user_input(4, 2, "second"),
        model_response(5, 2, "req_2", "answer2"),
        turn_end(6, 2),
        user_input(7, 3, "third"),
        model_response(8, 3, "req_3", "answer3"),
        handoff_event_with_keep_turns(9, 3, "summary", 2),
        turn_end(10, 3),
        user_input(11, 4, "fourth"),
        model_response(12, 4, "req_4", "answer4"),
        turn_end(13, 4),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);

    assert_eq!(summary.as_deref(), Some("summary"));
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("second"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer2".to_string())]),
            CanonicalMessage::user_text("third"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer3".to_string())]),
            CanonicalMessage::user_text("fourth"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer4".to_string())]),
        ]
    );
}
