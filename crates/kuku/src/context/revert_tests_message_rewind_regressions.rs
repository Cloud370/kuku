use super::{
    apply_file_revert, compute_message_rewind_plan, filter_rolled_back_events,
    message_rewind_file_read, message_rewind_file_write, ts, ui, FileRestore, RevertPlan,
};
use crate::event::{EventPayload, StoredEvent};
use serde_json::json;
use std::collections::HashSet;

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
    apply_file_revert(&plan, workspace.path(), session_dir.path(), 4).unwrap();

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
    apply_file_revert(&plan, workspace.path(), session_dir.path(), 5).unwrap();

    assert_eq!(
        "selected before",
        std::fs::read_to_string(selected).unwrap()
    );
    assert_eq!(
        "other Agent after",
        std::fs::read_to_string(unrelated).unwrap()
    );
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
