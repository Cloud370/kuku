fn ts(id: u64, turn: u64) -> StoredEvent {
    event(
        id,
        EventPayload::TurnStarted {
            execution: crate::event::test_execution_scope(),
            turn,
            ts: "t".to_string(),
            conversation: "main".to_string(),
        },
    )
}

fn rb(id: u64, _turn: u64, target_turn: u64, scope: RollbackScope) -> StoredEvent {
    event(
        id,
        EventPayload::ConversationRollback {
            ts: "t".to_string(),
            conversation: ConversationAddress::MAIN.as_str().to_string(),
            to_turn: target_turn,
            to_event_id: turn_end_id_for(target_turn),
            scope,
        },
    )
}

fn rb_undo(id: u64, _turn: u64, rb_id: u64) -> StoredEvent {
    event(
        id,
        EventPayload::ConversationRollbackUndone {
            ts: "t".to_string(),
            conversation: ConversationAddress::MAIN.as_str().to_string(),
            rollback_event_id: rb_id,
        },
    )
}

fn turn_end_id_for(turn: u64) -> u64 {
    turn * 3
}

fn et<'a>(events: &[&'a StoredEvent]) -> Vec<&'a str> {
    events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::MessageUser { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn no_rollback_returns_all() {
    let events = vec![ts(1, 1), user_input(2, 1, "a"), turn_end(3, 1)];
    assert_eq!(filter_rolled_back_events(&events).len(), 3);
}

#[test]
fn both_scope_skips_target_and_later_turns() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        ts(4, 2),
        user_input(5, 2, "b"),
        turn_end(6, 2),
        ts(7, 3),
        user_input(8, 3, "c"),
        turn_end(9, 3),
        rb(10, 4, 2, RollbackScope::Both),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(et(&f), vec!["a"]);
}

#[test]
fn messages_scope_skips_turns() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        ts(4, 2),
        user_input(5, 2, "b"),
        turn_end(6, 2),
        rb(7, 3, 2, RollbackScope::ConversationOnly),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(et(&f), vec!["a"]);
}

#[test]
fn file_changes_scope_keeps_conversation() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        ts(4, 2),
        user_input(5, 2, "b"),
        turn_end(6, 2),
        rb(7, 3, 2, RollbackScope::FilesOnly),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(et(&f), vec!["a", "b"]);
}

#[test]
fn undo_restores_events() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        ts(4, 2),
        user_input(5, 2, "b"),
        turn_end(6, 2),
        rb(7, 3, 2, RollbackScope::ConversationOnly),
        rb_undo(8, 4, 7),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(et(&f), vec!["a", "b"]);
}

#[test]
fn rollback_before_handoff_removes_handoff() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        handoff_event_with_keep_turns(4, 1, "old summary", 2),
        ts(5, 2),
        user_input(6, 2, "b"),
        turn_end(7, 2),
        rb(8, 3, 1, RollbackScope::ConversationOnly),
    ];
    let (summary, msgs) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert!(msgs.is_empty());
}

#[test]
fn rollback_after_handoff_keeps_summary() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        handoff_event_with_keep_turns(4, 1, "summary of turn 1", 2),
        ts(5, 2),
        user_input(6, 2, "b"),
        turn_end(7, 2),
        ts(8, 3),
        user_input(9, 3, "c"),
        turn_end(10, 3),
        rb(11, 4, 3, RollbackScope::ConversationOnly),
    ];
    let (summary, msgs) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(summary.as_deref(), Some("summary of turn 1"));
    let texts: Vec<_> = msgs
        .iter()
        .filter_map(|m| {
            if let MessageBlock::Text(t) = &m.blocks[0] {
                Some(t.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(texts, vec!["a", "b"]);
}

#[test]
fn consecutive_rollbacks_last_wins() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        ts(4, 2),
        user_input(5, 2, "b"),
        turn_end(6, 2),
        ts(7, 3),
        user_input(8, 3, "c"),
        turn_end(9, 3),
        rb(10, 4, 2, RollbackScope::ConversationOnly),
        rb(11, 5, 3, RollbackScope::ConversationOnly),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(et(&f), vec!["a", "b"]);
}

#[test]
fn undo_first_of_two_rollbacks_second_still_active() {
    let events = vec![
        ts(1, 1),
        user_input(2, 1, "a"),
        turn_end(3, 1),
        ts(4, 2),
        user_input(5, 2, "b"),
        turn_end(6, 2),
        ts(7, 3),
        user_input(8, 3, "c"),
        turn_end(9, 3),
        rb(10, 4, 2, RollbackScope::ConversationOnly),
        rb(11, 5, 3, RollbackScope::ConversationOnly),
        rb_undo(12, 6, 10),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(et(&f), vec!["a", "b"]);
}
