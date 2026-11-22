use super::*;
use serde_json::json;

fn ts(id: u64, turn: u64) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::TurnStarted {
            turn,
            ts: "t".into(),
            conversation: "main".into(),
        },
    }
}

fn te(id: u64, turn: u64) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::TurnCompleted {
            turn,
            ts: "t".into(),
            conversation: "main".into(),
        },
    }
}

fn ui(id: u64, turn: u64, text: &str) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::MessageUser {
            turn,
            ts: "t".into(),
            conversation: "main".into(),
            text: text.into(),
            from: None,
            via_tool_call_id: None,
            job_delivery: false,
        },
    }
}

fn tool_result_read(id: u64, turn: u64, path: &str, content: &str, full: bool) -> StoredEvent {
    tool_result_read_with_flags(id, turn, path, content, full, false)
}

fn tool_result_read_with_flags(
    id: u64,
    turn: u64,
    path: &str,
    content: &str,
    full: bool,
    cached: bool,
) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ToolResult {
            turn,
            ts: "t".into(),
            conversation: None,
            tool_call_id: "tc1".into(),
            status: "ok".into(),
            summary: String::new(),
            model_content: String::new(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: Some(json!({
                "kind": "file_content",
                "canonical_path": path,
                "raw_text": content,
                "cached": cached,
                "is_full_file_snapshot": full,
            })),
        },
    }
}

fn tr(id: u64, turn: u64, tc: &str, kind: &str, path: &str, content: &str) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ToolResult {
            turn,
            ts: "t".into(),
            conversation: None,
            tool_call_id: tc.into(),
            status: "ok".into(),
            summary: String::new(),
            model_content: String::new(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: Some(
                json!({"kind": kind, "canonical_path": path, "raw_text_after": content}),
            ),
        },
    }
}

fn tr_in(
    id: u64,
    turn: u64,
    conversation: &str,
    tc: &str,
    kind: &str,
    path: &str,
    content: &str,
) -> StoredEvent {
    let mut event = tr(id, turn, tc, kind, path, content);
    if let EventPayload::ToolResult {
        conversation: owner,
        ..
    } = &mut event.payload
    {
        *owner = Some(conversation.into());
    }
    event
}

fn message_rewind_file_read(
    id: u64,
    turn: u64,
    path: &str,
    canonical_path: &Path,
    content: &str,
) -> StoredEvent {
    message_rewind_file_read_with_flags(id, turn, path, canonical_path, content, true, false)
}

fn message_rewind_file_read_with_flags(
    id: u64,
    turn: u64,
    path: &str,
    canonical_path: &Path,
    content: &str,
    full: bool,
    cached: bool,
) -> StoredEvent {
    let mut event = tool_result_read_with_flags(id, turn, path, content, full, cached);
    if let EventPayload::ToolResult {
        structured: Some(value),
        ..
    } = &mut event.payload
    {
        value["canonical_path"] = json!(canonical_path.to_string_lossy());
        value["path"] = json!(path);
    }
    event
}

fn message_rewind_file_write(
    id: u64,
    turn: u64,
    conversation: &str,
    path: &str,
    canonical_path: &Path,
    content: &str,
    created: bool,
) -> StoredEvent {
    let mut event = tr_in(id, turn, conversation, "write", "file_write", path, content);
    if let EventPayload::ToolResult {
        structured: Some(value),
        ..
    } = &mut event.payload
    {
        value["canonical_path"] = json!(canonical_path.to_string_lossy());
        value["path"] = json!(path);
        value["created"] = json!(created);
    }
    event
}

fn rb(id: u64, _turn: u64, target: u64, scope: RollbackScope) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ConversationRollback {
            ts: "t".into(),
            conversation: "main".into(),
            to_turn: target,
            to_event_id: id.saturating_sub(1),
            scope,
        },
    }
}

fn rb_undo(id: u64, _turn: u64, rb_id: u64) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ConversationRollbackUndone {
            ts: "t".into(),
            conversation: "main".into(),
            rollback_event_id: rb_id,
        },
    }
}

fn message_rewind(id: u64, address: &str, target_event_id: u64) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::Unknown(json!({
            "kind": "runtime.operation",
            "record": {
                "operation": "message_rewind",
                "address": address,
                "target_event_id": target_event_id,
            }
        })),
    }
}

fn conversation_opened(id: u64, conversation: &str) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ConversationOpened {
            ts: "t".into(),
            conversation: conversation.into(),
        },
    }
}

fn message_user(id: u64, conversation: &str, turn: u64, text: &str) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::MessageUser {
            ts: "t".into(),
            conversation: conversation.into(),
            turn,
            text: text.into(),
            from: None,
            via_tool_call_id: None,
            job_delivery: false,
        },
    }
}

fn conversation_rb(id: u64, conversation: &str, target: u64, scope: RollbackScope) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ConversationRollback {
            ts: "t".into(),
            conversation: conversation.into(),
            to_turn: target,
            to_event_id: id.saturating_sub(1),
            scope,
        },
    }
}

fn conversation_rb_undo(id: u64, conversation: &str, rb_id: u64) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ConversationRollbackUndone {
            ts: "t".into(),
            conversation: conversation.into(),
            rollback_event_id: rb_id,
        },
    }
}

fn extract_user_texts<'a>(events: &[&'a StoredEvent]) -> Vec<&'a str> {
    events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::MessageUser { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn extract_message_user_texts<'a>(events: &[&'a StoredEvent]) -> Vec<&'a str> {
    events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::MessageUser { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// filter_rolled_back_events tests

#[test]
fn filter_no_rollback_returns_all() {
    let events = vec![ts(1, 1), ui(2, 1, "a"), te(3, 1)];
    assert_eq!(filter_rolled_back_events(&events).len(), 3);
}

#[test]
fn message_rewinds_hide_bounded_agent_intervals_without_hiding_new_inputs() {
    let mut other_agent = ui(8, 1, "other agent");
    if let EventPayload::MessageUser { conversation, .. } = &mut other_agent.payload {
        *conversation = "reviewer".into();
    }
    let events = vec![
        ts(1, 1),
        ui(2, 1, "first target"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "tail one"),
        te(6, 2),
        message_rewind(7, "main", 2),
        other_agent,
        ts(9, 3),
        ui(10, 3, "second target"),
        te(11, 3),
        ts(12, 4),
        ui(13, 4, "tail two"),
        te(14, 4),
        message_rewind(15, "main", 10),
        ts(16, 5),
        ui(17, 5, "new input"),
        te(18, 5),
    ];

    let visible: Vec<_> = filter_rolled_back_events(&events)
        .into_iter()
        .map(|event| event.id)
        .collect();

    assert!(!visible.contains(&2));
    assert!(!visible.contains(&5));
    assert!(!visible.contains(&10));
    assert!(!visible.contains(&13));
    assert!(visible.contains(&8));
    assert!(visible.contains(&17));
}

#[test]
fn filter_both_scope_skips_target_and_later_turns() {
    let events = vec![
        ts(1, 1),
        ui(2, 1, "a"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "b"),
        te(6, 2),
        ts(7, 3),
        ui(8, 3, "c"),
        te(9, 3),
        rb(10, 4, 2, RollbackScope::Both),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(extract_user_texts(&f), vec!["a"]);
}

#[test]
fn filter_messages_scope_skips_turns() {
    let events = vec![
        ts(1, 1),
        ui(2, 1, "a"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "b"),
        te(6, 2),
        rb(7, 3, 2, RollbackScope::ConversationOnly),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(extract_user_texts(&f), vec!["a"]);
}

#[test]
fn filter_file_changes_scope_keeps_conversation() {
    let events = vec![
        ts(1, 1),
        ui(2, 1, "a"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "b"),
        te(6, 2),
        rb(7, 3, 2, RollbackScope::FilesOnly),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(extract_user_texts(&f), vec!["a", "b"]);
}

#[test]
fn filter_undo_restores_events() {
    let events = vec![
        ts(1, 1),
        ui(2, 1, "a"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "b"),
        te(6, 2),
        rb(7, 3, 2, RollbackScope::ConversationOnly),
        rb_undo(8, 4, 7),
    ];
    let f = filter_rolled_back_events(&events);
    assert_eq!(extract_user_texts(&f), vec!["a", "b"]);
}

#[test]
fn filter_undo_latest_rollback_reactivates_previous_rollback() {
    let events = vec![
        ts(1, 1),
        ui(2, 1, "a"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "b"),
        te(6, 2),
        ts(7, 3),
        ui(8, 3, "c"),
        te(9, 3),
        rb(10, 4, 2, RollbackScope::ConversationOnly),
        rb(11, 5, 3, RollbackScope::ConversationOnly),
        rb_undo(12, 6, 11),
    ];

    let f = filter_rolled_back_events(&events);

    assert_eq!(extract_user_texts(&f), vec!["a"]);
}

#[test]
fn conversation_rollback_only_hides_target_conversation_messages() {
    let events = vec![
        conversation_opened(1, "main"),
        conversation_opened(2, "review/api"),
        conversation_opened(3, "explore"),
        message_user(4, "main", 1, "main-1"),
        message_user(5, "review/api", 1, "review-1"),
        message_user(6, "explore", 1, "explore-1"),
        message_user(7, "review/api", 2, "review-2"),
        conversation_rb(8, "review/api", 2, RollbackScope::ConversationOnly),
    ];

    let filtered = filter_rolled_back_events(&events);

    assert_eq!(
        extract_message_user_texts(&filtered),
        vec!["main-1", "review-1", "explore-1"]
    );
}

// compute_file_revert_plan tests

#[test]
fn file_modified_after_target_restores() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "v2").unwrap();
    let events = vec![
        ts(1, 1),
        tool_result_read(2, 1, "a.txt", "v1", true),
        te(3, 1),
        ts(4, 2),
        tr(5, 2, "tc2", "file_write", "a.txt", "v2"),
        te(6, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    assert_eq!(plan.restores.len(), 1);
    assert_eq!(plan.restores[0].old_content, "v1");
    assert!(plan.deletes.is_empty());
    apply_file_revert(&plan, dir.path(), session_dir.path(), 10).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "v1"
    );
}

#[test]
fn message_rewind_file_opt_in_restores_selected_agent_changes() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), "after").unwrap();
    let events = vec![
        ts(1, 1),
        message_rewind_file_read(2, 1, "a.txt", &workspace_root.join("a.txt"), "before"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "rewind target"),
        te(6, 2),
        ts(7, 3),
        message_rewind_file_write(
            8,
            3,
            "main",
            "a.txt",
            &workspace_root.join("a.txt"),
            "after",
            false,
        ),
        te(9, 3),
    ];

    let plan = compute_message_rewind_plan(&events, "main", 5, workspace.path()).unwrap();

    assert_eq!(plan.restores.len(), 1);
    let warnings = apply_file_revert(&plan, workspace.path(), session_dir.path(), 10).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(
        "before",
        std::fs::read_to_string(workspace.path().join("a.txt")).unwrap()
    );
}

#[test]
fn message_rewind_uses_full_read_during_selected_turn_as_before_image() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), "after").unwrap();
    let events = vec![
        ui(1, 1, "rewind target"),
        message_rewind_file_read(2, 1, "a.txt", &workspace_root.join("a.txt"), "before"),
        message_rewind_file_write(
            3,
            1,
            "main",
            "a.txt",
            &workspace_root.join("a.txt"),
            "after",
            false,
        ),
    ];

    let plan = compute_message_rewind_plan(&events, "main", 1, workspace.path()).unwrap();

    assert_eq!(plan.restores.len(), 1);
    assert_eq!("before", plan.restores[0].old_content);
    assert!(plan.unrecoverable.is_empty());
    apply_file_revert(&plan, workspace.path(), session_dir.path(), 10).unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("a.txt")).unwrap(),
        "before"
    );
}

#[test]
fn message_rewind_deletes_only_an_explicitly_created_file() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("new.txt"), "created").unwrap();
    let events = vec![
        ui(1, 1, "rewind target"),
        message_rewind_file_write(
            2,
            1,
            "main",
            "new.txt",
            &workspace_root.join("new.txt"),
            "created",
            true,
        ),
    ];

    let plan = compute_message_rewind_plan(&events, "main", 1, workspace.path()).unwrap();

    assert_eq!(plan.deletes, vec![workspace_root.join("new.txt")]);
    assert!(plan.unrecoverable.is_empty());
    apply_file_revert(&plan, workspace.path(), session_dir.path(), 10).unwrap();
    assert!(!workspace.path().join("new.txt").exists());
}

#[test]
fn message_rewind_treats_missing_before_image_as_unrecoverable() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    std::fs::write(workspace.path().join("unknown.txt"), "after").unwrap();
    let events = vec![
        ui(1, 1, "rewind target"),
        message_rewind_file_write(
            2,
            1,
            "main",
            "unknown.txt",
            &workspace_root.join("unknown.txt"),
            "after",
            false,
        ),
    ];

    let plan = compute_message_rewind_plan(&events, "main", 1, workspace.path()).unwrap();

    assert_eq!(plan.unrecoverable, vec![workspace_root.join("unknown.txt")]);
    assert!(plan.deletes.is_empty());
}

#[test]
fn message_rewind_file_plan_rejects_other_agent_later_writes() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let events = vec![
        ts(1, 1),
        message_rewind_file_read(
            2,
            1,
            "shared.txt",
            &workspace_root.join("shared.txt"),
            "before",
        ),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "rewind target"),
        te(6, 2),
        ts(7, 3),
        message_rewind_file_write(
            8,
            3,
            "main",
            "shared.txt",
            &workspace_root.join("shared.txt"),
            "main",
            false,
        ),
        te(9, 3),
        StoredEvent {
            id: 10,
            payload: EventPayload::TurnStarted {
                turn: 4,
                ts: "t".into(),
                conversation: "review".into(),
            },
        },
        message_rewind_file_write(
            11,
            4,
            "review",
            "shared.txt",
            &workspace_root.join("shared.txt"),
            "review",
            false,
        ),
    ];

    let error = compute_message_rewind_plan(&events, "main", 5, workspace.path())
        .err()
        .unwrap();

    assert!(error.to_string().contains("another Agent"));
}

#[cfg(unix)]
#[test]
fn message_rewind_file_plan_rejects_symlink_paths() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("file.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("link")).unwrap();
    let events = vec![
        ts(1, 1),
        ui(2, 1, "rewind target"),
        message_rewind_file_write(
            3,
            1,
            "main",
            "link/file.txt",
            &workspace_root.join("link/file.txt"),
            "after",
            true,
        ),
    ];

    let error = compute_message_rewind_plan(&events, "main", 2, workspace.path())
        .err()
        .unwrap();

    assert!(error.to_string().contains("symbolic link"));
    assert_eq!(
        "outside",
        std::fs::read_to_string(outside.path().join("file.txt")).unwrap()
    );
}

#[path = "revert_tests_message_rewind_regressions.rs"]
mod message_rewind_regressions;

#[test]
fn file_created_after_target_deletes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("new.txt"), "new").unwrap();
    let events = vec![
        ts(1, 1),
        te(2, 1),
        ts(3, 2),
        tr(4, 2, "tc2", "file_write", "new.txt", "new"),
        te(5, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    assert_eq!(plan.deletes.len(), 1);
    assert!(plan.restores.is_empty());
}

#[test]
fn partial_read_only_marks_unrecoverable() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.txt"), "changed").unwrap();
    let events = vec![
        ts(1, 1),
        tool_result_read(2, 1, "b.txt", "partial", false),
        te(3, 1),
        ts(4, 2),
        tr(5, 2, "tc2", "file_write", "b.txt", "changed"),
        te(6, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    assert_eq!(plan.unrecoverable.len(), 1);
    apply_file_revert(&plan, dir.path(), session_dir.path(), 10).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "changed"
    );
}

#[test]
fn memory_write_tracked_as_file_change() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("memory.md"), "new").unwrap();
    let events = vec![
        ts(1, 1),
        tool_result_read(2, 1, "memory.md", "old", true),
        te(3, 1),
        ts(4, 2),
        tr(5, 2, "tc3", "memory_write", "memory.md", "new"),
        te(6, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    assert_eq!(plan.restores.len(), 1);
    assert_eq!(plan.restores[0].old_content, "old");
}

#[test]
fn forget_memory_tracked_as_file_change() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("memory.md"), "new").unwrap();
    let events = vec![
        ts(1, 1),
        tool_result_read(2, 1, "memory.md", "old", true),
        te(3, 1),
        ts(4, 2),
        tr(5, 2, "tc4", "forget_memory", "memory.md", "new"),
        te(6, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    assert_eq!(plan.restores.len(), 1);
}

#[test]
fn no_changes_after_target_empty_plan() {
    let dir = tempfile::tempdir().unwrap();
    let events = vec![ts(1, 1), te(2, 1)];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    assert!(plan.restores.is_empty());
    assert!(plan.deletes.is_empty());
}

#[test]
fn sensitive_file_detected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "secret").unwrap();
    let events = vec![
        ts(1, 1),
        te(2, 1),
        ts(3, 2),
        tr(4, 2, "tc2", "file_write", ".env", "secret"),
        te(5, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    assert_eq!(plan.sensitive_files.len(), 1);
}

#[test]
fn untrusted_absolute_event_path_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "outside").unwrap();
    let outside_path = outside.path().to_string_lossy().into_owned();
    let events = vec![
        ts(1, 1),
        tool_result_read(2, 1, &outside_path, "before", true),
        te(3, 1),
        ts(4, 2),
        tr(5, 2, "tc2", "file_write", &outside_path, "after"),
        te(6, 2),
    ];

    let plan = compute_file_revert_plan(&events, 1, dir.path());

    assert!(plan.restores.is_empty());
    assert!(plan.deletes.is_empty());
    assert!(plan.unrecoverable.is_empty());
    assert!(plan.sensitive_files.is_empty());
}

#[test]
fn target_turn_zero_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let events = vec![ts(1, 1), te(2, 1)];
    let plan = compute_file_revert_plan(&events, 0, dir.path());
    assert!(plan.restores.is_empty());
}

// apply_file_revert tests

#[test]
fn apply_restores_file_content() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "v2").unwrap();
    let events = vec![
        ts(1, 1),
        tool_result_read(2, 1, "a.txt", "v1", true),
        te(3, 1),
        ts(4, 2),
        tr(5, 2, "tc2", "file_write", "a.txt", "v2"),
        te(6, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    let warnings = apply_file_revert(&plan, dir.path(), dir.path(), 99).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "v1"
    );
    assert!(dir.path().join("pre-revert-99").exists());
}

#[test]
fn apply_deletes_created_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("new.txt"), "new").unwrap();
    let events = vec![
        ts(1, 1),
        te(2, 1),
        ts(3, 2),
        tr(4, 2, "tc2", "file_write", "new.txt", "new"),
        te(5, 2),
    ];
    let plan = compute_file_revert_plan(&events, 1, dir.path());
    let _warnings = apply_file_revert(&plan, dir.path(), dir.path(), 99).unwrap();
    assert!(!dir.path().join("new.txt").exists());
    assert!(dir.path().join("pre-revert-99").join("new.txt").exists());
}

#[test]
fn apply_file_revert_rejects_paths_outside_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "outside").unwrap();
    let plan = RevertPlan {
        restores: vec![FileRestore {
            path: outside.path().to_path_buf(),
            old_content: "restored".into(),
            old_hash: String::new(),
            new_content_on_plan: "outside".into(),
        }],
        deletes: vec![],
        unrecoverable: vec![],
        sensitive_files: vec![],
    };

    let result = apply_file_revert(&plan, dir.path(), dir.path(), 99);

    assert!(result.is_err());
    assert_eq!("outside", std::fs::read_to_string(outside.path()).unwrap());
}

#[cfg(unix)]
#[test]
fn apply_file_revert_rejects_workspace_symlink_escape() {
    let dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    let outside_path = outside_dir.path().join("outside.txt");
    std::fs::write(&outside_path, "outside").unwrap();
    std::os::unix::fs::symlink(outside_dir.path(), dir.path().join("link")).unwrap();

    let plan = RevertPlan {
        restores: vec![FileRestore {
            path: dir.path().join("link/outside.txt"),
            old_content: "restored".into(),
            old_hash: String::new(),
            new_content_on_plan: "outside".into(),
        }],
        deletes: vec![],
        unrecoverable: vec![],
        sensitive_files: vec![],
    };

    let result = apply_file_revert(&plan, dir.path(), dir.path(), 99);

    assert!(result.is_err());
    assert_eq!("outside", std::fs::read_to_string(outside_path).unwrap());
}

// find_active_rollback tests

#[test]
fn active_rollback_found() {
    let events = vec![conversation_rb(10, "main", 3, RollbackScope::Both)];
    let active = find_active_rollback(&events).unwrap();
    assert_eq!(active.rollback_event_id, 10);
    assert_eq!(active.to_turn, 3);
}

#[test]
fn active_rollback_undone_returns_none() {
    let events = vec![
        conversation_rb(10, "main", 3, RollbackScope::Both),
        conversation_rb_undo(11, "main", 10),
    ];
    assert!(find_active_rollback(&events).is_none());
}

#[test]
fn undo_latest_rollback_reactivates_previous_rollback() {
    let events = vec![
        conversation_rb(10, "main", 2, RollbackScope::Both),
        conversation_rb(11, "main", 3, RollbackScope::ConversationOnly),
        conversation_rb_undo(12, "main", 11),
    ];

    let active = find_active_rollback(&events).unwrap();

    assert_eq!(active.rollback_event_id, 10);
    assert_eq!(active.to_turn, 2);
    assert_eq!(active.scope, RollbackScope::Both);
}

// list_user_turns tests

#[test]
fn user_turns_listed_reverse() {
    let events = vec![
        ts(1, 1),
        ui(2, 1, "first message"),
        te(3, 1),
        ts(4, 2),
        ui(5, 2, "second message"),
        te(6, 2),
    ];
    let turns = list_user_turns(&events);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].turn, 2);
    assert_eq!(turns[1].turn, 1);
    assert_eq!(turns[0].text_preview, "second message");
}

#[test]
fn user_turns_for_main_undo_ignore_child_conversations() {
    let events = vec![
        ts(1, 1),
        ui(2, 1, "main message"),
        te(3, 1),
        conversation_opened(4, "review"),
        message_user(5, "review", 2, "child message"),
    ];

    let turns = list_user_turns(&events);

    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn, 1);
    assert_eq!(turns[0].text_preview, "main message");
}

// count_file_turns_after tests

#[test]
fn count_file_turns_after_correct() {
    let events = vec![
        ts(1, 1),
        te(2, 1),
        ts(3, 2),
        tr(4, 2, "tc2", "file_write", "a.txt", "v1"),
        te(5, 2),
        ts(6, 3),
        tr(7, 3, "tc2", "file_write", "b.txt", "v2"),
        te(8, 3),
    ];
    assert_eq!(count_file_turns_after(&events, 1), 2);
    assert_eq!(count_file_turns_after(&events, 2), 1);
    assert_eq!(count_file_turns_after(&events, 3), 0);
}
