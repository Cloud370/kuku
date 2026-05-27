//! File revert plan computation and execution for turn rollbacks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::event::{EventPayload, RollbackScope, StoredEvent};

const REVERTABLE_KINDS: &[&str] = &["file_edit", "file_write", "memory_write", "forget_memory"];

/// Plan for reverting files to their state before a target turn.
pub struct RevertPlan {
    pub restores: Vec<FileRestore>,
    pub deletes: Vec<PathBuf>,
    pub unrecoverable: Vec<PathBuf>,
    pub sensitive_files: Vec<PathBuf>,
}

/// A single file to be restored to its previous content.
pub struct FileRestore {
    pub path: PathBuf,
    pub old_content: String,
    pub old_hash: String,
    pub new_content_on_plan: String,
}

/// The currently active (non-undone) rollback in a session.
pub struct ActiveRollback {
    pub rollback_event_id: u64,
    pub target_turn: u64,
    pub scope: RollbackScope,
}

/// A user turn entry for the interactive undo selection list.
pub struct UserTurnEntry {
    pub turn: u64,
    pub ts: String,
    pub text_preview: String,
    pub has_file_changes: bool,
}

enum FileStateAt {
    Exists(String),
    NotExists,
    Unrecoverable,
}

/// Filter events to exclude turns that have been rolled back (conversation scope).
pub fn filter_rolled_back_events(events: &[StoredEvent]) -> Vec<&StoredEvent> {
    let mut undone_ids: HashSet<u64> = HashSet::new();
    let mut active_rollback: Option<(u64, u64, RollbackScope)> = None;

    for event in events {
        match &event.payload {
            EventPayload::TurnRollbackUndo {
                rollback_event_id, ..
            } => {
                undone_ids.insert(*rollback_event_id);
                if active_rollback
                    .as_ref()
                    .is_some_and(|(_, rb_id, _)| rb_id == rollback_event_id)
                {
                    active_rollback = None;
                }
            }
            EventPayload::TurnRollback {
                target_turn, scope, ..
            } if !undone_ids.contains(&event.id) => {
                active_rollback = Some((*target_turn, event.id, scope.clone()));
            }
            _ => {}
        }
    }

    let skipped_turns: HashSet<u64> = match &active_rollback {
        Some((target_turn, _, scope)) if scope.affects_conversation() => events
            .iter()
            .filter_map(|e| {
                if let EventPayload::TurnStart { turn, .. } = &e.payload {
                    if *turn >= *target_turn {
                        Some(*turn)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect(),
        _ => HashSet::new(),
    };

    let mut current_turn: Option<u64> = None;
    events
        .iter()
        .filter(|event| match &event.payload {
            EventPayload::TurnStart { turn, .. } => {
                current_turn = Some(*turn);
                !skipped_turns.contains(turn)
            }
            EventPayload::TurnEnd { turn, .. }
            | EventPayload::UserInput { turn, .. }
            | EventPayload::ModelResponse { turn, .. }
            | EventPayload::ToolCall { turn, .. }
            | EventPayload::ToolResult { turn, .. }
            | EventPayload::ModelRequest { turn, .. }
            | EventPayload::ModelError { turn, .. }
            | EventPayload::PermissionRequest { turn, .. }
            | EventPayload::PermissionDecision { turn, .. } => !skipped_turns.contains(turn),
            EventPayload::Handoff { .. } | EventPayload::HandoffTrigger { .. } => {
                match current_turn {
                    Some(t) => !skipped_turns.contains(&t),
                    None => true,
                }
            }
            _ => true,
        })
        .collect()
}

/// Compute which files need to be reverted to restore state before a target turn.
pub fn compute_file_revert_plan(
    events: &[StoredEvent],
    target_turn: u64,
    workspace: &Path,
) -> RevertPlan {
    if target_turn == 0 {
        return RevertPlan {
            restores: vec![],
            deletes: vec![],
            unrecoverable: vec![],
            sensitive_files: vec![],
        };
    }
    let turn_end_pos = find_turn_end_pos(events, target_turn);

    let mut modified_files: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, event) in events.iter().enumerate() {
        if i <= turn_end_pos {
            continue;
        }
        if let EventPayload::ToolResult {
            structured: Some(s),
            ..
        } = &event.payload
        {
            if let Some(kind) = s["kind"].as_str() {
                if REVERTABLE_KINDS.contains(&kind) {
                    if let Some(path) = s["canonical_path"].as_str().or_else(|| s["path"].as_str())
                    {
                        modified_files.entry(path.to_string()).or_default().push(i);
                    }
                }
            }
        }
    }

    let mut restores = Vec::new();
    let mut deletes = Vec::new();
    let mut unrecoverable = Vec::new();
    let mut sensitive_files = Vec::new();

    for file_path in modified_files.keys() {
        if is_system_dir_path(file_path) {
            continue;
        }
        let target_state = find_file_state_at(events, file_path, turn_end_pos);
        let disk_path = workspace.join(file_path);

        let file_name = disk_path.file_name().unwrap_or_default().to_string_lossy();
        if crate::tool::builtin::common::is_sensitive_file_name(&file_name) {
            sensitive_files.push(disk_path.clone());
        }

        match target_state {
            FileStateAt::Exists(old_content) => {
                let old_hash = crate::tool::builtin::common::content_hash(old_content.as_bytes());
                let new_content = std::fs::read_to_string(&disk_path).unwrap_or_default();
                if old_content != new_content {
                    restores.push(FileRestore {
                        path: disk_path,
                        old_content,
                        old_hash,
                        new_content_on_plan: new_content,
                    });
                }
            }
            FileStateAt::NotExists => {
                if disk_path.exists() {
                    deletes.push(disk_path);
                }
            }
            FileStateAt::Unrecoverable => {
                unrecoverable.push(disk_path);
            }
        }
    }

    RevertPlan {
        restores,
        deletes,
        unrecoverable,
        sensitive_files,
    }
}

fn is_system_dir_path(path: &str) -> bool {
    let normalized = crate::tool::builtin::common::normalize_path_sep(path);
    normalized
        .split('/')
        .any(|part| part == ".git" || part == ".ssh")
}

fn find_turn_end_pos(events: &[StoredEvent], target_turn: u64) -> usize {
    if let Some(pos) = events.iter().rposition(
        |e| matches!(&e.payload, EventPayload::TurnEnd { turn, .. } if *turn == target_turn),
    ) {
        return pos;
    }
    if let Some(pos) = events.iter().position(|e| {
        matches!(
            &e.payload,
            EventPayload::TurnStart { turn, .. } if *turn > target_turn
        )
    }) {
        return pos.saturating_sub(1);
    }
    events.len().saturating_sub(1)
}

fn find_file_state_at(events: &[StoredEvent], canonical_path: &str, at_pos: usize) -> FileStateAt {
    for event in events[..=at_pos].iter().rev() {
        if let EventPayload::ToolResult {
            structured: Some(s),
            ..
        } = &event.payload
        {
            if s["kind"] == "file_content"
                && s["cached"] == false
                && s["canonical_path"].as_str() == Some(canonical_path)
                && s["is_full_file_snapshot"].as_bool().unwrap_or(false)
            {
                if let Some(text) = s["raw_text_after"].as_str() {
                    return FileStateAt::Exists(text.to_string());
                }
            }
        }
    }

    let mut found_read = false;
    for event in events[..=at_pos].iter().rev() {
        if let EventPayload::ToolResult {
            structured: Some(s),
            ..
        } = &event.payload
        {
            let path_match = s["canonical_path"].as_str() == Some(canonical_path)
                || s["path"].as_str() == Some(canonical_path);
            if !path_match {
                continue;
            }
            match s["kind"].as_str() {
                Some("file_content") => {
                    found_read = true;
                }
                Some(kind) if REVERTABLE_KINDS.contains(&kind) && !found_read => {
                    return FileStateAt::NotExists;
                }
                _ => {}
            }
        }
    }
    if found_read {
        FileStateAt::Unrecoverable
    } else {
        FileStateAt::NotExists
    }
}

/// Find the most recent non-undone rollback in the event stream.
pub fn find_active_rollback(events: &[StoredEvent]) -> Option<ActiveRollback> {
    let mut undone_ids: HashSet<u64> = HashSet::new();
    let mut last: Option<ActiveRollback> = None;
    for event in events {
        match &event.payload {
            EventPayload::TurnRollbackUndo {
                rollback_event_id, ..
            } => {
                undone_ids.insert(*rollback_event_id);
                if last
                    .as_ref()
                    .is_some_and(|r| r.rollback_event_id == *rollback_event_id)
                {
                    last = None;
                }
            }
            EventPayload::TurnRollback {
                target_turn, scope, ..
            } if !undone_ids.contains(&event.id) => {
                last = Some(ActiveRollback {
                    rollback_event_id: event.id,
                    target_turn: *target_turn,
                    scope: scope.clone(),
                });
            }
            _ => {}
        }
    }
    last
}

/// List user turns in reverse chronological order for undo selection.
pub fn list_user_turns(events: &[StoredEvent]) -> Vec<UserTurnEntry> {
    let mut current_turn: Option<u64> = None;
    let mut file_turns: HashSet<u64> = HashSet::new();
    for event in events {
        if let EventPayload::TurnStart { turn, .. } = &event.payload {
            current_turn = Some(*turn);
        }
        if let EventPayload::ToolResult {
            structured: Some(s),
            ..
        } = &event.payload
        {
            if let Some(kind) = s["kind"].as_str() {
                if REVERTABLE_KINDS.contains(&kind) {
                    if let Some(t) = current_turn {
                        file_turns.insert(t);
                    }
                }
            }
        }
    }
    let mut turns: Vec<UserTurnEntry> = events
        .iter()
        .filter_map(|e| {
            if let EventPayload::UserInput { turn, ts, text } = &e.payload {
                Some(UserTurnEntry {
                    turn: *turn,
                    ts: ts.clone(),
                    text_preview: text.chars().take(50).collect(),
                    has_file_changes: file_turns.contains(turn),
                })
            } else {
                None
            }
        })
        .collect();
    turns.reverse();
    turns
}

/// Count turns with file modifications after a given turn.
pub fn count_file_turns_after(events: &[StoredEvent], after_turn: u64) -> usize {
    let mut file_turns: HashSet<u64> = HashSet::new();
    let mut current_turn: Option<u64> = None;
    for event in events {
        if let EventPayload::TurnStart { turn, .. } = &event.payload {
            current_turn = Some(*turn);
        }
        if current_turn.is_some_and(|t| t > after_turn) {
            if let EventPayload::ToolResult {
                structured: Some(s),
                ..
            } = &event.payload
            {
                if let Some(kind) = s["kind"].as_str() {
                    if REVERTABLE_KINDS.contains(&kind) {
                        if let Some(t) = current_turn {
                            file_turns.insert(t);
                        }
                    }
                }
            }
        }
    }
    file_turns.len()
}

/// Execute a file revert plan: backup, restore, and delete files.
pub fn apply_file_revert(
    plan: &RevertPlan,
    workspace: &Path,
    session_dir: &Path,
    rollback_event_id: u64,
) -> Result<Vec<String>, std::io::Error> {
    let backup_dir = session_dir.join(format!("pre-revert-{rollback_event_id}"));
    let mut warnings = Vec::new();

    for restore in &plan.restores {
        if restore.path.exists() {
            if let Ok(relative) = restore.path.strip_prefix(workspace) {
                let backup_path = backup_dir.join(relative);
                if let Some(parent) = backup_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&restore.path, &backup_path)?;
            }
        }
    }
    for delete_path in &plan.deletes {
        if delete_path.exists() {
            if let Ok(relative) = delete_path.strip_prefix(workspace) {
                let backup_path = backup_dir.join(relative);
                if let Some(parent) = backup_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(delete_path, &backup_path)?;
            }
        }
    }

    for restore in &plan.restores {
        if restore.path.exists() {
            let current = std::fs::read(&restore.path).unwrap_or_default();
            let current_hash = crate::tool::builtin::common::content_hash(&current);
            if current_hash
                != crate::tool::builtin::common::content_hash(
                    restore.new_content_on_plan.as_bytes(),
                )
            {
                warnings.push(format!(
                    "{}: file changed since plan computed",
                    restore.path.display()
                ));
            }
        }
        crate::tool::builtin::common::write_atomically(
            &restore.path,
            restore.old_content.as_bytes(),
        )?;
    }

    for delete_path in &plan.deletes {
        if delete_path.exists() {
            std::fs::remove_file(delete_path)?;
        }
    }

    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ts(id: u64, turn: u64) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::TurnStart {
                turn,
                ts: "t".into(),
            },
        }
    }

    fn te(id: u64, turn: u64) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::TurnEnd {
                turn,
                ts: "t".into(),
            },
        }
    }

    fn ui(id: u64, turn: u64, text: &str) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::UserInput {
                turn,
                ts: "t".into(),
                text: text.into(),
            },
        }
    }

    fn tool_result_read(id: u64, turn: u64, path: &str, content: &str, full: bool) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::ToolResult {
                turn,
                ts: "t".into(),
                tool_call_id: "tc1".into(),
                status: "ok".into(),
                summary: String::new(),
                model_content: String::new(),
                truncated: false,
                structured: Some(json!({
                    "kind": "file_content",
                    "canonical_path": path,
                    "raw_text_after": content,
                    "cached": false,
                    "is_full_file_snapshot": full,
                    "content_hash": "sha256:abc",
                    "start_line": 1,
                    "line_count": content.lines().count(),
                })),
            },
        }
    }

    fn tool_result_write(id: u64, turn: u64, path: &str, content: &str) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::ToolResult {
                turn,
                ts: "t".into(),
                tool_call_id: "tc2".into(),
                status: "ok".into(),
                summary: String::new(),
                model_content: String::new(),
                truncated: false,
                structured: Some(json!({
                    "kind": "file_write",
                    "canonical_path": path,
                    "raw_text_after": content,
                    "content_hash_after": "sha256:def",
                })),
            },
        }
    }

    fn tool_result_memory_write(id: u64, turn: u64, path: &str, content: &str) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::ToolResult {
                turn,
                ts: "t".into(),
                tool_call_id: "tc3".into(),
                status: "ok".into(),
                summary: String::new(),
                model_content: String::new(),
                truncated: false,
                structured: Some(json!({
                    "kind": "memory_write",
                    "canonical_path": path,
                    "raw_text_after": content,
                    "content_hash_after": "sha256:ghi",
                })),
            },
        }
    }

    fn tool_result_forget_memory(id: u64, turn: u64, path: &str, content: &str) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::ToolResult {
                turn,
                ts: "t".into(),
                tool_call_id: "tc4".into(),
                status: "ok".into(),
                summary: String::new(),
                model_content: String::new(),
                truncated: false,
                structured: Some(json!({
                    "kind": "forget_memory",
                    "canonical_path": path,
                    "raw_text_after": content,
                    "content_hash_after": "sha256:jkl",
                })),
            },
        }
    }

    fn rb(id: u64, turn: u64, target: u64, scope: RollbackScope) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::TurnRollback {
                turn,
                ts: "t".into(),
                target_turn: target,
                scope,
            },
        }
    }

    fn rb_undo(id: u64, turn: u64, rb_id: u64) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::TurnRollbackUndo {
                turn,
                ts: "t".into(),
                rollback_event_id: rb_id,
            },
        }
    }

    fn extract_user_texts<'a>(events: &[&'a StoredEvent]) -> Vec<&'a str> {
        events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::UserInput { text, .. } => Some(text.as_str()),
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
    fn filter_conversation_only_skips_turns() {
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
    fn filter_files_only_keeps_conversation() {
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

    // compute_file_revert_plan tests

    #[test]
    fn file_modified_after_target_restores() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "v2").unwrap();
        let events = vec![
            ts(1, 1),
            tool_result_read(2, 1, "a.txt", "v1", true),
            te(3, 1),
            ts(4, 2),
            tool_result_write(5, 2, "a.txt", "v2"),
            te(6, 2),
        ];
        let plan = compute_file_revert_plan(&events, 1, dir.path());
        assert_eq!(plan.restores.len(), 1);
        assert_eq!(plan.restores[0].old_content, "v1");
        assert!(plan.deletes.is_empty());
    }

    #[test]
    fn file_created_after_target_deletes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("new.txt"), "new").unwrap();
        let events = vec![
            ts(1, 1),
            te(2, 1),
            ts(3, 2),
            tool_result_write(4, 2, "new.txt", "new"),
            te(5, 2),
        ];
        let plan = compute_file_revert_plan(&events, 1, dir.path());
        assert_eq!(plan.deletes.len(), 1);
        assert!(plan.restores.is_empty());
    }

    #[test]
    fn partial_read_only_marks_unrecoverable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "content").unwrap();
        let events = vec![
            ts(1, 1),
            tool_result_read(2, 1, "b.txt", "partial", false),
            te(3, 1),
            ts(4, 2),
            tool_result_write(5, 2, "b.txt", "changed"),
            te(6, 2),
        ];
        let plan = compute_file_revert_plan(&events, 1, dir.path());
        assert_eq!(plan.unrecoverable.len(), 1);
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
            tool_result_memory_write(5, 2, "memory.md", "new"),
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
            tool_result_forget_memory(5, 2, "memory.md", "new"),
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
            tool_result_write(4, 2, ".env", "secret"),
            te(5, 2),
        ];
        let plan = compute_file_revert_plan(&events, 1, dir.path());
        assert_eq!(plan.sensitive_files.len(), 1);
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
            tool_result_write(5, 2, "a.txt", "v2"),
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
            tool_result_write(4, 2, "new.txt", "new"),
            te(5, 2),
        ];
        let plan = compute_file_revert_plan(&events, 1, dir.path());
        let _warnings = apply_file_revert(&plan, dir.path(), dir.path(), 99).unwrap();
        assert!(!dir.path().join("new.txt").exists());
        assert!(dir.path().join("pre-revert-99").join("new.txt").exists());
    }

    // find_active_rollback tests

    #[test]
    fn active_rollback_found() {
        let events = vec![rb(10, 5, 3, RollbackScope::Both)];
        let active = find_active_rollback(&events).unwrap();
        assert_eq!(active.rollback_event_id, 10);
        assert_eq!(active.target_turn, 3);
    }

    #[test]
    fn active_rollback_undone_returns_none() {
        let events = vec![rb(10, 5, 3, RollbackScope::Both), rb_undo(11, 6, 10)];
        assert!(find_active_rollback(&events).is_none());
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

    // count_file_turns_after tests

    #[test]
    fn count_file_turns_after_correct() {
        let events = vec![
            ts(1, 1),
            te(2, 1),
            ts(3, 2),
            tool_result_write(4, 2, "a.txt", "v1"),
            te(5, 2),
            ts(6, 3),
            tool_result_write(7, 3, "b.txt", "v2"),
            te(8, 3),
        ];
        assert_eq!(count_file_turns_after(&events, 1), 2);
        assert_eq!(count_file_turns_after(&events, 2), 1);
        assert_eq!(count_file_turns_after(&events, 3), 0);
    }
}
