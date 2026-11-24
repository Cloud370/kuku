use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::event::EventStore;
use crate::event::store::JournalSummary;
use crate::session::{AgentStatus, Session, SessionStatus};

#[derive(Debug, Clone)]
/// Summary metadata for a session in a workspace.
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

/// A discovered Session journal and the workspace containing it.
pub struct SessionFile {
    session_id: String,
    workspace: PathBuf,
    path: PathBuf,
    mtime: String,
    size: u64,
}

/// The Session picker projection, built from a single journal read or a live Session.
pub struct SessionListing {
    pub summary: SessionSummary,
    pub waiting_agents: Vec<String>,
}

impl SessionFile {
    pub fn events_path(&self) -> &Path {
        &self.path
    }

    pub fn listing(&self, open: Option<&Session>) -> Result<SessionListing> {
        let (facts, agents, pending_title) = Session::listing(&self.path, open)?;
        let mut summary = self.summary(facts);
        if summary.title.is_empty() {
            summary.title = pending_title.unwrap_or_default();
        }
        if agents
            .iter()
            .any(|agent| agent.status == AgentStatus::Running)
        {
            summary.status = SessionStatus::Active;
        } else if agents.iter().any(|agent| {
            matches!(
                agent.status,
                AgentStatus::Interrupted | AgentStatus::Stopped
            )
        }) {
            summary.status = SessionStatus::Interrupted;
        } else if agents.iter().any(|agent| agent.turn.is_some()) {
            summary.status = SessionStatus::Done;
        }
        let waiting_agents = agents
            .into_iter()
            .filter(|agent| agent.status == AgentStatus::Waiting)
            .map(|agent| agent.address)
            .collect();
        Ok(SessionListing {
            summary,
            waiting_agents,
        })
    }

    fn summary(&self, facts: JournalSummary) -> SessionSummary {
        SessionSummary {
            session_id: self.session_id.clone(),
            workspace: self.workspace.clone(),
            title: facts.title.unwrap_or_default(),
            created_at: facts.created_at,
            turn_count: facts.turns,
            status: session_status(&self.path.with_file_name("lock"), facts.main_completed),
            mtime: self.mtime.clone(),
            size: self.size,
        }
    }
}

/// List sessions, optionally filtered to a single workspace.
/// When workspace is None, walks all workspaces under `p/`.
/// Sorted by mtime descending (most recent first).
pub fn list_sessions(kuku_home: &Path, workspace: Option<&Path>) -> Result<Vec<SessionSummary>> {
    Ok(session_files(kuku_home, workspace)?
        .into_iter()
        .map(|file| {
            file.summary(EventStore::journal_summary(file.events_path()).unwrap_or_default())
        })
        .collect())
}

/// Discover Sessions in newest-first order without reading their event records.
pub fn session_files(kuku_home: &Path, workspace: Option<&Path>) -> Result<Vec<SessionFile>> {
    let mut files = Vec::new();

    if let Some(ws) = workspace {
        let sessions_dir = super::paths::project_home(kuku_home, ws)?.join("sessions");
        collect_sessions(&mut files, &sessions_dir, ws)?;
    } else {
        let p_dir = kuku_home.join("p");
        if p_dir.is_dir() {
            find_sessions_dirs(&mut files, &p_dir, &p_dir)?;
        }
    };

    files.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    Ok(files)
}

fn collect_sessions(
    files: &mut Vec<SessionFile>,
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
        let (mtime, size) = file_stat(&events_path);

        files.push(SessionFile {
            session_id,
            workspace: workspace.to_path_buf(),
            path: events_path,
            mtime,
            size,
        });
    }
    Ok(())
}

fn session_status(lock_path: &Path, completed: bool) -> SessionStatus {
    if let Ok(content) = fs::read_to_string(lock_path) {
        let pid_str = content.lines().next().unwrap_or("");
        if let Ok(pid) = pid_str.parse::<i32>() {
            if super::process_alive(pid) {
                return SessionStatus::Active;
            }
        }
    }
    if completed {
        SessionStatus::Done
    } else {
        SessionStatus::Interrupted
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

fn find_sessions_dirs(files: &mut Vec<SessionFile>, p_dir: &Path, current: &Path) -> Result<()> {
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
            let _ = collect_sessions(files, &path, &workspace);
        } else {
            find_sessions_dirs(files, p_dir, &path)?;
        }
    }
    Ok(())
}
