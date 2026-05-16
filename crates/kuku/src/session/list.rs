use std::path::Path;

use crate::error::Result;
use crate::event::{EventPayload, EventStore, StoredEvent};

#[derive(Debug, Clone)]
/// Summary metadata for a listed session.
pub struct SessionSummary {
    pub session_id: String,
    pub created_at: String,
    pub turn_count: u64,
}

/// List all sessions in a workspace, sorted by creation time.
pub fn list_sessions(kuku_home: &Path, workspace: &Path) -> Result<Vec<SessionSummary>> {
    let sessions_dir = super::paths::project_home(kuku_home, workspace)?.join("sessions");
    let mut summaries = Vec::new();

    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(summaries),
    };

    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let events_path = entry.path().join("events.jsonl");
        let events = match EventStore::replay(&events_path) {
            Ok(events) => events,
            Err(_) => continue,
        };
        let session_id = session_id_from_meta(&events)
            .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
        let created_at = created_at_from_meta(&events).unwrap_or_default();
        let turn_count = count_turns(&events);
        summaries.push(SessionSummary {
            session_id,
            created_at,
            turn_count,
        });
    }
    Ok(summaries)
}

fn session_id_from_meta(events: &[StoredEvent]) -> Option<String> {
    events.first().and_then(|e| match &e.payload {
        EventPayload::SessionMeta { session_id, .. } => Some(session_id.clone()),
        _ => None,
    })
}

fn created_at_from_meta(events: &[StoredEvent]) -> Option<String> {
    events.first().and_then(|e| match &e.payload {
        EventPayload::SessionMeta { created_at, .. } => Some(created_at.clone()),
        _ => None,
    })
}

fn count_turns(events: &[StoredEvent]) -> u64 {
    events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::TurnStart { .. }))
        .count() as u64
}
