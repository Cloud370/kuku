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
            job_delivery: false,
        },
    }
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

// find_active_rollback tests

// list_user_turns tests

// count_file_turns_after tests
