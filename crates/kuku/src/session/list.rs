use std::fs;
use std::path::{Path, PathBuf};

use crate::conversation::address::ConversationAddress;
use crate::conversation::{reduce_conversations, TurnTerminal};
use crate::error::Result;
use crate::event;
use crate::session::SessionStatus;

#[derive(Debug, Clone)]
/// Summary metadata for a session, extracted from disk without full event replay.
pub struct SessionSummary {
    pub session_id: String,
    pub workspace: PathBuf,
    pub title: String,
    pub created_at: String,
    pub turn_count: u64,
    pub status: SessionStatus,
    pub mtime: String,
    pub size: u64,
}

/// List sessions, optionally filtered to a single workspace.
/// When workspace is None, walks all workspaces under `p/`.
/// Sorted by mtime descending (most recent first).
pub fn list_sessions(kuku_home: &Path, workspace: Option<&Path>) -> Result<Vec<SessionSummary>> {
    let mut summaries = Vec::new();

    if let Some(ws) = workspace {
        let sessions_dir = super::paths::project_home(kuku_home, ws)?.join("sessions");
        collect_sessions(&mut summaries, &sessions_dir, ws)?;
    } else {
        let p_dir = kuku_home.join("p");
        if p_dir.is_dir() {
            collect_global_sessions(&mut summaries, &p_dir)?;
        }
    };

    summaries.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    Ok(summaries)
}

fn collect_sessions(
    summaries: &mut Vec<SessionSummary>,
    sessions_dir: &Path,
    workspace: &Path,
) -> Result<()> {
    let entries = match fs::read_dir(sessions_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let events_path = entry.path().join("events.jsonl");
        if !events_path.exists() {
            continue;
        }
        let session_id = entry.file_name().to_string_lossy().into_owned();
        let title = event::scan::scan_first_user_input(&events_path).unwrap_or_default();
        let created_at = event::scan::scan_session_meta(&events_path).unwrap_or_default();
        let turn_count = event::scan::scan_turn_count(&events_path);
        let events = event::EventStore::replay(&events_path).unwrap_or_default();
        let lock_path = entry.path().join("lock");
        let status = session_status(&lock_path, &events);

        let (mtime, size) = file_stat(&events_path);

        summaries.push(SessionSummary {
            session_id,
            workspace: workspace.to_path_buf(),
            title,
            created_at,
            turn_count,
            status,
            mtime,
            size,
        });
    }
    Ok(())
}

fn session_status(lock_path: &Path, events: &[event::StoredEvent]) -> SessionStatus {
    if let Ok(content) = fs::read_to_string(lock_path) {
        let pid_str = content.lines().next().unwrap_or("");
        if let Ok(pid) = pid_str.parse::<i32>() {
            if super::process_alive(pid) {
                return SessionStatus::Active;
            }
        }
    }
    let main = reduce_conversations(events)
        .into_iter()
        .find(|conversation| conversation.address == ConversationAddress::MAIN);
    match main {
        Some(conversation) if conversation.active_turn.is_some() => SessionStatus::Interrupted,
        Some(conversation)
            if matches!(
                conversation.last_terminal,
                Some((_, TurnTerminal::Completed))
            ) =>
        {
            SessionStatus::Done
        }
        Some(conversation)
            if matches!(
                conversation.last_terminal,
                Some((_, TurnTerminal::Interrupted))
            ) =>
        {
            SessionStatus::Interrupted
        }
        _ => match event::scan::scan_last_event_type(
            &lock_path.parent().unwrap().join("events.jsonl"),
        ) {
            Some("turn.end") => SessionStatus::Done,
            _ => SessionStatus::Interrupted,
        },
    }
}

fn file_stat(path: &Path) -> (String, u64) {
    match fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| {
                    let dt: time::OffsetDateTime = t.into();
                    dt.format(&time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_default();
            (mtime, meta.len())
        }
        Err(_) => (String::new(), 0),
    }
}

fn collect_global_sessions(summaries: &mut Vec<SessionSummary>, p_dir: &Path) -> Result<()> {
    find_sessions_dirs(summaries, p_dir, p_dir)
}

fn find_sessions_dirs(
    summaries: &mut Vec<SessionSummary>,
    p_dir: &Path,
    current: &Path,
) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|n| n == "sessions") {
            let project_home = path.parent().unwrap();
            let workspace = super::paths::workspace_from_project_home(p_dir, project_home);
            let _ = collect_sessions(summaries, &path, &workspace);
        } else {
            find_sessions_dirs(summaries, p_dir, &path)?;
        }
    }
    Ok(())
}
