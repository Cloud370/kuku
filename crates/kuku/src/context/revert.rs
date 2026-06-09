//! File revert plan computation and execution for turn rollbacks.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, EventStore, RollbackScope, StoredEvent};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRollback {
    pub conversation: String,
    pub rollback_event_id: u64,
    pub to_turn: u64,
    pub to_event_id: u64,
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
    let conversation_rollbacks = active_conversation_rollbacks(events);

    events
        .iter()
        .filter(|event| {
            if let Some(conversation) = event_conversation_key(&event.payload) {
                if conversation_rollbacks
                    .get(conversation)
                    .is_some_and(|rollback| {
                        event_hidden_by_rollback(&event.payload, event.id, rollback)
                    })
                {
                    return false;
                }
            }

            match &event.payload {
                EventPayload::ContextPrelude { .. }
                | EventPayload::SessionCreated { .. }
                | EventPayload::ConversationOpened { .. }
                | EventPayload::ConversationBound { .. }
                | EventPayload::PromptSnapshot { .. }
                | EventPayload::MessageUser { .. }
                | EventPayload::MessageAssistant { .. }
                | EventPayload::TurnStarted { .. }
                | EventPayload::TurnCompleted { .. }
                | EventPayload::TurnCancelled { .. }
                | EventPayload::TurnInterrupted { .. }
                | EventPayload::ConversationRollback { .. }
                | EventPayload::ConversationRollbackUndone { .. } => true,
                EventPayload::SessionMeta { .. }
                | EventPayload::TurnStart { .. }
                | EventPayload::TurnEnd { .. }
                | EventPayload::UserInput { .. }
                | EventPayload::ModelResponse { .. }
                | EventPayload::ToolCall { .. }
                | EventPayload::ToolResult { .. }
                | EventPayload::ModelError { .. }
                | EventPayload::PermissionRequested { .. }
                | EventPayload::PermissionAllow { .. }
                | EventPayload::PermissionDeny { .. }
                | EventPayload::ContextSources { .. }
                | EventPayload::ContextSkills { .. }
                | EventPayload::Handoff { .. }
                | EventPayload::TurnRollback { .. }
                | EventPayload::TurnRollbackUndo { .. }
                | EventPayload::Unknown(_) => true,
            }
        })
        .collect()
}

fn active_conversation_rollbacks(events: &[StoredEvent]) -> HashMap<String, ActiveRollback> {
    let mut undone_ids = HashSet::new();
    for event in events {
        if let EventPayload::ConversationRollbackUndone {
            rollback_event_id, ..
        } = &event.payload
        {
            undone_ids.insert(*rollback_event_id);
        }
    }

    let mut active = HashMap::new();
    for event in events.iter().rev() {
        match &event.payload {
            EventPayload::ConversationRollback {
                conversation,
                to_turn,
                to_event_id,
                scope,
                ..
            } => {
                if !scope.affects_conversation() || undone_ids.contains(&event.id) {
                    continue;
                }
                active
                    .entry(conversation.clone())
                    .or_insert(ActiveRollback {
                        conversation: conversation.clone(),
                        rollback_event_id: event.id,
                        to_turn: *to_turn,
                        to_event_id: *to_event_id,
                        scope: scope.clone(),
                    });
            }
            EventPayload::TurnRollback {
                target_turn, scope, ..
            } => {
                if !scope.affects_conversation() || undone_ids.contains(&event.id) {
                    continue;
                }
                active
                    .entry(ConversationAddress::MAIN.as_str().to_string())
                    .or_insert(ActiveRollback {
                        conversation: ConversationAddress::MAIN.as_str().to_string(),
                        rollback_event_id: event.id,
                        to_turn: *target_turn,
                        to_event_id: find_turn_boundary_event_id(
                            events,
                            ConversationAddress::MAIN.as_str(),
                            *target_turn,
                        )
                        .unwrap_or(event.id),
                        scope: scope.clone(),
                    });
            }
            _ => {}
        }
    }
    active
}

fn event_hidden_by_rollback(
    payload: &EventPayload,
    event_id: u64,
    rollback: &ActiveRollback,
) -> bool {
    if !rollback.scope.affects_conversation() {
        return false;
    }
    if let Some(turn) = conversation_event_turn(payload) {
        if rollback.conversation == ConversationAddress::MAIN.as_str()
            && conversation_event_conversation(payload).is_none()
        {
            return turn >= rollback.to_turn;
        }
        return turn >= rollback.to_turn;
    }
    event_id > rollback.to_event_id
}

fn conversation_event_turn(payload: &EventPayload) -> Option<u64> {
    match payload {
        EventPayload::PromptSnapshot { turn, .. }
        | EventPayload::MessageUser { turn, .. }
        | EventPayload::MessageAssistant { turn, .. }
        | EventPayload::TurnStarted { turn, .. }
        | EventPayload::TurnCompleted { turn, .. }
        | EventPayload::TurnCancelled { turn, .. }
        | EventPayload::TurnInterrupted { turn, .. }
        | EventPayload::TurnStart { turn, .. }
        | EventPayload::UserInput { turn, .. }
        | EventPayload::ModelResponse { turn, .. }
        | EventPayload::ModelError { turn, .. }
        | EventPayload::ToolCall { turn, .. }
        | EventPayload::PermissionAllow { turn, .. }
        | EventPayload::PermissionRequested { turn, .. }
        | EventPayload::PermissionDeny { turn, .. }
        | EventPayload::ToolResult { turn, .. }
        | EventPayload::Handoff { turn, .. }
        | EventPayload::TurnEnd { turn, .. }
        | EventPayload::ContextSources { turn, .. }
        | EventPayload::ContextSkills { turn, .. }
        | EventPayload::TurnRollback { turn, .. }
        | EventPayload::TurnRollbackUndo { turn, .. } => Some(*turn),
        EventPayload::ConversationOpened { .. }
        | EventPayload::ConversationBound { .. }
        | EventPayload::ConversationRollback { .. }
        | EventPayload::ConversationRollbackUndone { .. }
        | EventPayload::SessionMeta { .. }
        | EventPayload::ContextPrelude { .. }
        | EventPayload::SessionCreated { .. }
        | EventPayload::Unknown(_) => None,
    }
}

fn conversation_event_conversation(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::ConversationOpened { conversation, .. }
        | EventPayload::ConversationBound { conversation, .. }
        | EventPayload::PromptSnapshot { conversation, .. }
        | EventPayload::MessageUser { conversation, .. }
        | EventPayload::MessageAssistant { conversation, .. }
        | EventPayload::TurnStarted { conversation, .. }
        | EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. }
        | EventPayload::ConversationRollback { conversation, .. }
        | EventPayload::ConversationRollbackUndone { conversation, .. } => {
            Some(conversation.as_str())
        }
        _ => None,
    }
}

fn event_conversation_key(payload: &EventPayload) -> Option<&str> {
    conversation_event_conversation(payload).or_else(|| {
        if matches!(
            payload,
            EventPayload::TurnStart { .. }
                | EventPayload::UserInput { .. }
                | EventPayload::ModelResponse { .. }
                | EventPayload::ModelError { .. }
                | EventPayload::ToolCall {
                    conversation: None,
                    ..
                }
                | EventPayload::ToolResult {
                    conversation: None,
                    ..
                }
                | EventPayload::PermissionRequested { .. }
                | EventPayload::PermissionAllow { .. }
                | EventPayload::PermissionDeny { .. }
                | EventPayload::ContextSources { .. }
                | EventPayload::ContextSkills { .. }
                | EventPayload::Handoff { .. }
                | EventPayload::TurnEnd { .. }
        ) {
            Some(ConversationAddress::MAIN.as_str())
        } else {
            None
        }
    })
}

fn find_turn_boundary_event_id(
    events: &[StoredEvent],
    conversation: &str,
    turn: u64,
) -> Option<u64> {
    events
        .iter()
        .rev()
        .find(|event| {
            conversation_event_turn(&event.payload) == Some(turn)
                && event_conversation_key(&event.payload) == Some(conversation)
        })
        .map(|event| event.id)
}

/// Compute which files need to be reverted to restore state before a target turn.
pub fn compute_file_revert_plan(
    events: &[StoredEvent],
    target_turn: u64,
    workspace: &Path,
) -> RevertPlan {
    let canonical_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
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
        let Some(disk_path) = resolve_revert_path(&canonical_workspace, file_path) else {
            continue;
        };
        let target_state = find_file_state_at(events, file_path, turn_end_pos);

        let file_name = disk_path.file_name().unwrap_or_default().to_string_lossy();
        if crate::util::path::is_sensitive_file_name(&file_name) {
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
    let normalized = crate::util::path::normalize_path_sep(path);
    normalized
        .split('/')
        .any(|part| part == ".git" || part == ".ssh")
}

fn resolve_revert_path(workspace: &Path, event_path: &str) -> Option<PathBuf> {
    let candidate = Path::new(event_path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    canonicalize_revert_target(workspace, &joined).ok()
}

fn canonicalize_revert_target(workspace: &Path, path: &Path) -> Result<PathBuf, std::io::Error> {
    let normalized =
        crate::tool::builtin::common::normalize_existing_components(path.to_path_buf());
    let mut existing = normalized.as_path();
    let mut suffix = Vec::<OsString>::new();

    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("path outside workspace: {}", normalized.display()),
            ));
        };
        suffix.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("path outside workspace: {}", normalized.display()),
            )
        })?;
    }

    let mut resolved = existing.canonicalize()?;
    while let Some(component) = suffix.pop() {
        resolved.push(component);
    }

    if resolved.starts_with(workspace) {
        Ok(resolved)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("path outside workspace: {}", resolved.display()),
        ))
    }
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
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ConversationRollbackUndone {
            rollback_event_id, ..
        } => Some(*rollback_event_id),
        _ => None,
    });
    let active = active_conversation_rollbacks(events);
    events.iter().rev().find_map(|event| {
        active
            .values()
            .find(|rollback| rollback.rollback_event_id == event.id)
            .cloned()
    })
}

fn collect_file_turns(events: &[StoredEvent], after_turn: Option<u64>) -> HashSet<u64> {
    let mut file_turns: HashSet<u64> = HashSet::new();
    let mut current_turn: Option<u64> = None;
    for event in events {
        if let EventPayload::TurnStart { turn, .. } = &event.payload {
            current_turn = Some(*turn);
        }
        if after_turn.is_some_and(|n| current_turn.is_some_and(|t| t <= n)) {
            continue;
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
    file_turns
}

/// List user turns in reverse chronological order for undo selection.
pub fn list_user_turns(events: &[StoredEvent]) -> Vec<UserTurnEntry> {
    let file_turns = collect_file_turns(events, None);
    let mut turns: Vec<UserTurnEntry> = events
        .iter()
        .filter_map(|e| {
            if let EventPayload::UserInput { turn, ts, text } = &e.payload {
                Some(UserTurnEntry {
                    turn: *turn,
                    ts: ts.clone(),
                    text_preview: text.lines().next().unwrap_or("").chars().take(50).collect(),
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
    collect_file_turns(events, Some(after_turn)).len()
}

/// Execute a file revert plan: backup, restore, and delete files.
pub fn apply_file_revert(
    plan: &RevertPlan,
    workspace: &Path,
    session_dir: &Path,
    rollback_event_id: u64,
) -> Result<Vec<String>, std::io::Error> {
    let canonical_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let backup_dir = session_dir.join(format!("pre-revert-{rollback_event_id}"));
    let mut warnings = Vec::new();

    for restore in &plan.restores {
        ensure_path_within_workspace(&canonical_workspace, &restore.path)?;
        if restore.path.exists() {
            if let Ok(relative) = restore.path.strip_prefix(&canonical_workspace) {
                let backup_path = backup_dir.join(relative);
                if let Some(parent) = backup_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&restore.path, &backup_path)?;
            }
        }
    }
    for delete_path in &plan.deletes {
        ensure_path_within_workspace(&canonical_workspace, delete_path)?;
        if delete_path.exists() {
            if let Ok(relative) = delete_path.strip_prefix(&canonical_workspace) {
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

fn ensure_path_within_workspace(workspace: &Path, path: &Path) -> Result<(), std::io::Error> {
    canonicalize_revert_target(workspace, path).map(|_| ())
}

/// Result of a rollback operation.
pub struct RollbackResult {
    /// ID of the appended rollback event.
    pub rollback_event_id: u64,
    /// Number of files restored to their previous content.
    pub files_restored: usize,
    /// Number of files deleted (were created after target turn).
    pub files_deleted: usize,
    /// Non-fatal warnings (e.g., file changed since plan computed).
    pub warnings: Vec<String>,
}

/// Result of an undo-rollback operation.
pub struct UndoRollbackResult {
    /// ID of the rollback event that was undone.
    pub rollback_event_id: u64,
    /// Whether files were actually restored from backup.
    pub files_restored: bool,
    /// Whether the conversation was restored (always true).
    pub conversation_restored: bool,
    /// Non-fatal warnings (e.g., files skipped due to safety rules).
    pub warnings: Vec<String>,
}

/// Roll back a conversation turn and optionally revert files.
pub fn rollback_turn(
    events_path: &Path,
    workspace: &Path,
    session_dir: &Path,
    conversation: &ConversationAddress,
    target_turn: u64,
    scope: RollbackScope,
) -> crate::Result<RollbackResult> {
    let events = EventStore::replay(events_path)?;
    let to_event_id = find_turn_boundary_event_id(&events, conversation.as_str(), target_turn)
        .ok_or_else(|| {
            crate::error::Error::InvalidArgument(format!("turn {target_turn} not found"))
        })?;

    let affects_files = scope.affects_files();
    let mut store = EventStore::open(events_path)?;
    let rb_event = store.append(EventPayload::ConversationRollback {
        ts: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default(),
        conversation: conversation.as_str().to_string(),
        to_turn: target_turn,
        to_event_id,
        scope,
    })?;

    let mut files_restored = 0;
    let mut files_deleted = 0;
    let mut warnings = Vec::new();
    if affects_files {
        let plan = compute_file_revert_plan(&events, target_turn, workspace);
        files_restored = plan.restores.len();
        files_deleted = plan.deletes.len();
        let w = apply_file_revert(&plan, workspace, session_dir, rb_event.id)?;
        warnings = w;
    }

    Ok(RollbackResult {
        rollback_event_id: rb_event.id,
        files_restored,
        files_deleted,
        warnings,
    })
}

/// Undo an active rollback: restore files from backup and append a rollback undo event.
pub fn undo_rollback(
    events_path: &Path,
    workspace: &Path,
    session_dir: &Path,
) -> crate::Result<UndoRollbackResult> {
    let check_events = EventStore::replay(events_path)?;
    let active = find_active_rollback(&check_events).ok_or(
        crate::error::Error::InvalidArgument("no active rollback found".into()),
    )?;

    let file_turn_count = count_file_turns_after(&check_events, active.to_turn);

    let restore_files = match &active.scope {
        RollbackScope::ConversationOnly => true,
        RollbackScope::FilesOnly => {
            if file_turn_count > 0 {
                return Err(crate::error::Error::InvalidArgument(format!(
                    "cannot undo files_only rollback: {file_turn_count} turn(s) with file changes since rollback target"
                )));
            }
            true
        }
        RollbackScope::Both => file_turn_count == 0,
    };

    let mut files_restored = false;
    let mut warnings = Vec::new();
    if restore_files && active.scope.affects_files() {
        let backup_dir = session_dir.join(format!("pre-revert-{}", active.rollback_event_id));
        if backup_dir.exists() {
            restore_dir_recursive(&backup_dir, &backup_dir, workspace)?;
            files_restored = true;
        } else {
            warnings.push(format!(
                "backup directory not found: pre-revert-{}",
                active.rollback_event_id
            ));
        }
    } else if active.scope == RollbackScope::Both && file_turn_count > 0 {
        warnings.push(format!(
            "{file_turn_count} turn(s) with file changes since rollback; files kept as-is"
        ));
    }
    let mut store = EventStore::open(events_path)?;
    store.append(EventPayload::ConversationRollbackUndone {
        ts: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default(),
        conversation: active.conversation.clone(),
        rollback_event_id: active.rollback_event_id,
    })?;

    Ok(UndoRollbackResult {
        rollback_event_id: active.rollback_event_id,
        files_restored,
        conversation_restored: true,
        warnings,
    })
}

fn restore_dir_recursive(
    base: &Path,
    current: &Path,
    workspace: &Path,
) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            restore_dir_recursive(base, &path, workspace)?;
        } else {
            let relative = path
                .strip_prefix(base)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            let target = canonicalize_revert_target(workspace, &workspace.join(relative))?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "revert_tests.rs"]
mod tests;
