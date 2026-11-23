use super::{
    apply_message_rewind_files, compute_message_rewind_plan, filter_rolled_back_events,
    message_rewind_file_read, message_rewind_file_read_with_flags, message_rewind_file_write,
    plan_file_rewind, te, ts, ui, FileRestore, RevertPlan,
};
use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::tool::{builtin, ToolResultEnvelope};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;

fn tool_result_event(
    id: u64,
    turn: u64,
    conversation: &ConversationAddress,
    tool_call_id: &str,
    result: ToolResultEnvelope,
) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ToolResult {
            turn,
            ts: "t".into(),
            conversation: Some(conversation.as_str().into()),
            tool_call_id: tool_call_id.into(),
            status: result.status,
            summary: result.summary,
            model_content: result.model_content,
            truncated: result.truncated,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: result.structured,
        },
    }
}

fn never_cancelled() -> bool {
    false
}

fn assert_real_tool_rewind(workspace_alias: &Path, session_dir: &Path) {
    let conversation = ConversationAddress::MAIN;
    let file_alias = workspace_alias.join("rewind.txt");
    std::fs::write(&file_alias, b"before\n").unwrap();
    let canonical_file = file_alias.canonicalize().unwrap();
    let mut events = vec![ts(1, 1), ui(2, 1, "rewind real file tools")];

    let read_before = builtin::read_file(
        &json!({"path": "rewind.txt"}),
        workspace_alias,
        &conversation,
        &events,
        3,
        &crate::RuntimeLimits::default(),
        &never_cancelled,
    );
    assert_eq!(read_before.status, "ok");
    let read_snapshot = read_before.structured.as_ref().unwrap();
    assert_eq!(read_snapshot["raw_text"], "before\n");
    assert_eq!(read_snapshot["cached"], false);
    assert_eq!(read_snapshot["is_full_file_snapshot"], true);
    assert_eq!(
        read_snapshot["canonical_path"].as_str(),
        Some(canonical_file.to_string_lossy().as_ref())
    );
    events.push(tool_result_event(
        3,
        1,
        &conversation,
        "read-before",
        read_before,
    ));

    let write = builtin::write_file(
        &json!({"path": "rewind.txt", "content": "after write\n"}),
        workspace_alias,
        &conversation,
        &events,
        crate::WorkspaceIoLimits::default(),
        &never_cancelled,
    );
    assert_eq!(write.status, "ok");
    assert_eq!(
        write.structured.as_ref().unwrap()["canonical_path"].as_str(),
        Some(canonical_file.to_string_lossy().as_ref())
    );
    events.push(tool_result_event(4, 1, &conversation, "write", write));

    let read_after = builtin::read_file(
        &json!({"path": "rewind.txt"}),
        workspace_alias,
        &conversation,
        &events,
        5,
        &crate::RuntimeLimits::default(),
        &never_cancelled,
    );
    assert_eq!(read_after.status, "ok");
    assert_eq!(
        read_after.structured.as_ref().unwrap()["canonical_path"].as_str(),
        Some(canonical_file.to_string_lossy().as_ref())
    );
    events.push(tool_result_event(
        5,
        1,
        &conversation,
        "read-after",
        read_after,
    ));

    let edit = builtin::edit_file(
        &json!({
            "path": "rewind.txt",
            "old_text": "after write",
            "new_text": "after edit",
        }),
        workspace_alias,
        &conversation,
        &events,
        crate::WorkspaceIoLimits::default().text_bytes,
        &never_cancelled,
    );
    assert_eq!(edit.status, "ok");
    assert_eq!(
        edit.structured.as_ref().unwrap()["canonical_path"].as_str(),
        Some(canonical_file.to_string_lossy().as_ref())
    );
    events.push(tool_result_event(6, 1, &conversation, "edit", edit));

    let journal = session_dir.join("events.jsonl");
    let mut store = EventStore::open(&journal).unwrap();
    store
        .append_batch(|_| Ok(events.into_iter().map(|event| event.payload).collect()))
        .unwrap();
    drop(store);
    let preview = plan_file_rewind(&journal, workspace_alias, "main", 2).unwrap();
    assert_eq!(preview.preview.changes[0].lines, Some((1, 1)));
    assert!(preview.preview.skipped.is_empty());
    let plan = preview.plan;
    assert_eq!(
        plan.restores.len(),
        1,
        "deletes={:?}, unrecoverable={:?}",
        plan.deletes,
        plan.unrecoverable
    );
    assert_eq!(plan.restores[0].path, canonical_file);
    assert_eq!(plan.restores[0].old_content, "before\n");
    assert_eq!(plan.restores[0].new_content_on_plan, "after edit\n");
    assert!(plan.deletes.is_empty());
    assert!(plan.unrecoverable.is_empty());
    apply_message_rewind_files(&plan, workspace_alias, session_dir, 7).unwrap();

    assert_eq!(std::fs::read(&canonical_file).unwrap(), b"before\n");
    assert_eq!(std::fs::read(&file_alias).unwrap(), b"before\n");
    assert_eq!(
        std::fs::read(session_dir.join("pre-revert-7/rewind.txt")).unwrap(),
        b"after edit\n"
    );
}

fn assert_real_mutation_backed_rewind(workspace_alias: &Path, session_dir: &Path) {
    let conversation = ConversationAddress::MAIN;
    let file_alias = workspace_alias.join("mutation-rewind.txt");
    std::fs::write(&file_alias, b"before\n").unwrap();
    let canonical_file = file_alias.canonicalize().unwrap();
    let mut events = vec![ts(1, 1), ui(2, 1, "write prior snapshot")];

    let read_before = builtin::read_file(
        &json!({"path": "mutation-rewind.txt"}),
        workspace_alias,
        &conversation,
        &events,
        3,
        &crate::RuntimeLimits::default(),
        &never_cancelled,
    );
    assert_eq!(read_before.status, "ok");
    events.push(tool_result_event(
        3,
        1,
        &conversation,
        "prior-read-before-write",
        read_before,
    ));

    let write = builtin::write_file(
        &json!({"path": "mutation-rewind.txt", "content": "after write\n"}),
        workspace_alias,
        &conversation,
        &events,
        crate::WorkspaceIoLimits::default(),
        &never_cancelled,
    );
    assert_eq!(write.status, "ok");
    assert_eq!(
        write.structured.as_ref().unwrap()["canonical_path"].as_str(),
        Some(canonical_file.to_string_lossy().as_ref())
    );
    assert_eq!(
        write.structured.as_ref().unwrap()["raw_text_after"],
        "after write\n"
    );
    events.push(tool_result_event(4, 1, &conversation, "prior-write", write));

    let read_before_edit = builtin::read_file(
        &json!({"path": "mutation-rewind.txt"}),
        workspace_alias,
        &conversation,
        &events,
        5,
        &crate::RuntimeLimits::default(),
        &never_cancelled,
    );
    assert_eq!(read_before_edit.status, "ok");
    events.push(tool_result_event(
        5,
        1,
        &conversation,
        "prior-read-before-edit",
        read_before_edit,
    ));

    let edit = builtin::edit_file(
        &json!({
            "path": "mutation-rewind.txt",
            "old_text": "after write",
            "new_text": "after edit",
        }),
        workspace_alias,
        &conversation,
        &events,
        crate::WorkspaceIoLimits::default().text_bytes,
        &never_cancelled,
    );
    assert_eq!(edit.status, "ok");
    assert_eq!(
        edit.structured.as_ref().unwrap()["canonical_path"].as_str(),
        Some(canonical_file.to_string_lossy().as_ref())
    );
    assert_eq!(
        edit.structured.as_ref().unwrap()["raw_text_after"],
        "after edit\n"
    );
    events.push(tool_result_event(6, 1, &conversation, "prior-edit", edit));

    let read_after_edit = builtin::read_file(
        &json!({"path": "mutation-rewind.txt"}),
        workspace_alias,
        &conversation,
        &events,
        7,
        &crate::RuntimeLimits::default(),
        &never_cancelled,
    );
    assert_eq!(read_after_edit.status, "ok");
    events.push(tool_result_event(
        7,
        1,
        &conversation,
        "prior-read-after-edit",
        read_after_edit,
    ));
    events.push(te(8, 1));
    events.push(ts(9, 2));
    events.push(ui(10, 2, "rewind write"));

    let target_write = builtin::write_file(
        &json!({"path": "mutation-rewind.txt", "content": "after target\n"}),
        workspace_alias,
        &conversation,
        &events,
        crate::WorkspaceIoLimits::default(),
        &never_cancelled,
    );
    assert_eq!(target_write.status, "ok");
    events.push(tool_result_event(
        11,
        2,
        &conversation,
        "target-write",
        target_write,
    ));

    let plan = compute_message_rewind_plan(&events, "main", 10, workspace_alias).unwrap();
    assert_eq!(
        plan.restores.len(),
        1,
        "unrecoverable={:?}",
        plan.unrecoverable
    );
    assert_eq!(plan.restores[0].old_content, "after edit\n");
    assert_eq!(plan.restores[0].new_content_on_plan, "after target\n");
    apply_message_rewind_files(&plan, workspace_alias, session_dir, 12).unwrap();
    assert_eq!(std::fs::read(&file_alias).unwrap(), b"after edit\n");
}

fn assert_message_rewind_ignores_invalid_read(full: bool, cached: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let file = workspace_root.join("a.txt");
    std::fs::write(&file, "changed").unwrap();
    let events = vec![
        ui(1, 1, "rewind target"),
        message_rewind_file_read_with_flags(
            2,
            1,
            "a.txt",
            &file,
            "not a safe before-image",
            full,
            cached,
        ),
        message_rewind_file_write(3, 1, "main", "a.txt", &file, "changed", false),
    ];
    let plan = compute_message_rewind_plan(&events, "main", 1, workspace.path()).unwrap();
    assert!(plan.restores.is_empty());
    assert_eq!(plan.unrecoverable, vec![file.clone()]);
    apply_message_rewind_files(&plan, workspace.path(), session_dir.path(), 10).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"changed");
}

#[test]
fn partial_message_rewind_read_is_not_a_before_image() {
    assert_message_rewind_ignores_invalid_read(false, false);
}

#[test]
fn cached_message_rewind_read_is_not_a_before_image() {
    assert_message_rewind_ignores_invalid_read(true, true);
}

fn assert_real_created_file_deletion(workspace_alias: &Path, session_dir: &Path) {
    let conversation = ConversationAddress::MAIN;
    let canonical_file = workspace_alias
        .canonicalize()
        .unwrap()
        .join("created-by-tool.txt");
    let mut events = vec![ts(1, 1), ui(2, 1, "remove created file")];
    let write = builtin::write_file(
        &json!({"path": "created-by-tool.txt", "content": "created\n"}),
        workspace_alias,
        &conversation,
        &events,
        crate::WorkspaceIoLimits::default(),
        &never_cancelled,
    );
    assert_eq!(write.status, "ok");
    let structured = write.structured.as_ref().unwrap();
    assert_eq!(structured["created"], true);
    assert_eq!(
        structured["canonical_path"].as_str(),
        Some(canonical_file.to_string_lossy().as_ref())
    );
    events.push(tool_result_event(
        3,
        1,
        &conversation,
        "created-write",
        write,
    ));

    let plan = compute_message_rewind_plan(&events, "main", 2, workspace_alias).unwrap();
    assert_eq!(plan.deletes, vec![canonical_file.clone()]);
    assert!(plan.unrecoverable.is_empty());
    apply_message_rewind_files(&plan, workspace_alias, session_dir, 4).unwrap();
    assert!(!canonical_file.exists());
}

#[test]
fn message_rewind_projects_unowned_answers_and_runtime_facts_to_their_agent() {
    let response = |id, turn, request_id: &str, text: &str| StoredEvent {
        id,
        payload: EventPayload::ModelResponse {
            conversation: None,
            turn,
            ts: "t".into(),
            request_id: request_id.into(),
            text: text.into(),
            thinking: None,
            stop_reason: None,
            input_tokens_total: None,
            output_tokens_total: None,
        },
    };
    let runtime_fact = |id, address: &str, operation: &str| StoredEvent {
        id,
        payload: EventPayload::Unknown(json!({
            "kind": "runtime.operation",
            "record": {"operation": operation, "address": address}
        })),
    };
    let events = vec![
        ts(1, 1),
        StoredEvent {
            id: 2,
            payload: EventPayload::TurnStarted {
                turn: 2,
                ts: "t".into(),
                conversation: "review".into(),
            },
        },
        response(3, 2, "review-before", "review answer before"),
        response(4, 1, "main-before", "main answer before"),
        ui(5, 1, "selected main input"),
        response(6, 1, "main-after", "main answer after"),
        response(7, 2, "review-after", "review answer after"),
        runtime_fact(8, "review", "agent_created"),
        runtime_fact(9, "main", "agent_created"),
        runtime_fact(10, "review", "question_replied"),
        runtime_fact(11, "main", "question_replied"),
        runtime_fact(12, "review", "permission_replied"),
        runtime_fact(13, "main", "permission_replied"),
        runtime_fact(14, "review", "turn_settled"),
        runtime_fact(15, "main", "turn_settled"),
        StoredEvent {
            id: 16,
            payload: EventPayload::Unknown(json!({
                "kind": "runtime.operation",
                "record": {"operation": "message_rewind", "address": "main", "target_event_id": 5}
            })),
        },
    ];

    let visible_ids: HashSet<_> = filter_rolled_back_events(&events)
        .into_iter()
        .map(|event| event.id)
        .collect();

    assert!(visible_ids.contains(&3));
    assert!(visible_ids.contains(&4));
    assert!(!visible_ids.contains(&6));
    assert!(visible_ids.contains(&7));
    assert!(visible_ids.contains(&8));
    assert!(!visible_ids.contains(&9));
    assert!(visible_ids.contains(&10));
    assert!(!visible_ids.contains(&11));
    assert!(visible_ids.contains(&12));
    assert!(!visible_ids.contains(&13));
    assert!(visible_ids.contains(&14));
    assert!(!visible_ids.contains(&15));
    assert!(visible_ids.contains(&16));
}

#[test]
fn message_rewind_prefers_canonical_path_for_agent_subdirectory() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let subdir = workspace_root.join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    std::fs::write(workspace_root.join("config.txt"), "root").unwrap();
    std::fs::write(subdir.join("config.txt"), "after").unwrap();
    let events = vec![
        ui(1, 1, "rewind target"),
        message_rewind_file_read(2, 1, "config.txt", &subdir.join("config.txt"), "before"),
        message_rewind_file_write(
            3,
            1,
            "main",
            "config.txt",
            &subdir.join("config.txt"),
            "after",
            false,
        ),
    ];

    let plan = compute_message_rewind_plan(&events, "main", 1, workspace.path()).unwrap();
    apply_message_rewind_files(&plan, workspace.path(), session_dir.path(), 4).unwrap();

    assert_eq!(
        "root",
        std::fs::read_to_string(workspace_root.join("config.txt")).unwrap()
    );
    assert_eq!(
        "before",
        std::fs::read_to_string(subdir.join("config.txt")).unwrap()
    );
}

#[test]
fn message_rewind_ignores_disjoint_external_agent_workspace_changes() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_root = workspace.path().canonicalize().unwrap();
    let external_workspace = tempfile::tempdir().unwrap();
    let external_root = external_workspace.path().canonicalize().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let selected = workspace_root.join("selected.txt");
    let unrelated = external_root.join("unrelated.txt");
    std::fs::write(&selected, "selected after").unwrap();
    std::fs::write(&unrelated, "other Agent after").unwrap();
    let events = vec![
        ui(1, 1, "rewind target"),
        message_rewind_file_read(2, 1, "selected.txt", &selected, "selected before"),
        message_rewind_file_write(
            3,
            1,
            "main",
            "selected.txt",
            &selected,
            "selected after",
            false,
        ),
        message_rewind_file_write(
            4,
            2,
            "reviewer",
            "unrelated.txt",
            &unrelated,
            "other Agent after",
            false,
        ),
    ];

    let plan = compute_message_rewind_plan(&events, "main", 1, workspace.path()).unwrap();
    apply_message_rewind_files(&plan, workspace.path(), session_dir.path(), 5).unwrap();

    assert_eq!(
        "selected before",
        std::fs::read_to_string(selected).unwrap()
    );
    assert_eq!(
        "other Agent after",
        std::fs::read_to_string(unrelated).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn message_rewind_accepts_a_symlink_workspace_with_real_file_tool_events() {
    use std::os::unix::fs::symlink;

    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let workspace_alias = parent.path().join("workspace-alias");
    symlink(&workspace, &workspace_alias).unwrap();
    assert_ne!(workspace_alias, workspace_alias.canonicalize().unwrap());
    let session_dir = tempfile::tempdir().unwrap();

    assert_real_tool_rewind(&workspace_alias, session_dir.path());
}

#[test]
fn message_rewind_with_real_file_tool_events_restores_in_a_direct_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();

    assert_real_tool_rewind(workspace.path(), session_dir.path());
}

#[test]
fn message_rewind_restores_from_real_prior_write_and_edit_snapshots() {
    let workspace = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();

    assert_real_mutation_backed_rewind(workspace.path(), session_dir.path());
}

#[test]
fn message_rewind_deletes_a_file_created_by_the_real_write_tool() {
    let workspace = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();

    assert_real_created_file_deletion(workspace.path(), session_dir.path());
}

#[cfg(target_os = "macos")]
#[test]
fn message_rewind_accepts_macos_temp_aliases_with_real_file_tool_events() {
    for parent in ["/tmp", "/var/tmp"] {
        let workspace = tempfile::Builder::new()
            .prefix("kuku-message-rewind-")
            .tempdir_in(parent)
            .unwrap();
        let workspace_alias = workspace.path();
        assert_ne!(workspace_alias, workspace_alias.canonicalize().unwrap());
        let session_dir = tempfile::tempdir().unwrap();

        assert_real_tool_rewind(workspace_alias, session_dir.path());
    }
}

#[test]
fn message_rewind_rolls_back_partial_file_apply_failure() {
    let workspace = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let first = workspace.path().join("first.txt");
    let second = workspace.path().join("second.txt");
    std::fs::write(&first, "first after").unwrap();
    std::fs::write(&second, "second after").unwrap();
    let plan = RevertPlan {
        restores: vec![
            FileRestore {
                path: first.clone(),
                old_content: "first before".into(),
                old_hash: String::new(),
                new_content_on_plan: "first after".into(),
            },
            FileRestore {
                path: second.clone(),
                old_content: "second before".into(),
                old_hash: String::new(),
                new_content_on_plan: "second after".into(),
            },
        ],
        deletes: Vec::new(),
        unrecoverable: Vec::new(),
        sensitive_files: Vec::new(),
    };

    let error = super::super::apply::apply_message_rewind_files_inner(
        &plan,
        workspace.path(),
        session_dir.path(),
        9,
        Some(1),
    )
    .unwrap_err();

    assert!(error.to_string().contains("injected file restore failure"));
    assert_eq!("first after", std::fs::read_to_string(first).unwrap());
    assert_eq!("second after", std::fs::read_to_string(second).unwrap());
}
