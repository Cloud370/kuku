use std::collections::HashMap;
use std::path::Path;

use crate::event::{EventPayload, StoredEvent};

use super::{resolve_revert_path, FileStateAt, REVERTABLE_KINDS};

pub(super) fn event_turn_owners(events: &[StoredEvent]) -> HashMap<u64, String> {
    let mut owners = HashMap::new();
    for event in events {
        if let (Some(turn), Some(owner)) = (
            super::conversation_event_turn(&event.payload),
            super::conversation_event_conversation(&event.payload),
        ) {
            owners.insert(turn, owner.to_owned());
        }
    }
    owners
}

pub(super) fn event_owner<'a>(
    event: &'a StoredEvent,
    turn_owners: &'a HashMap<u64, String>,
) -> Option<&'a str> {
    if let Some(owner) = super::conversation_event_conversation(&event.payload) {
        return Some(owner);
    }
    if let EventPayload::Unknown(value) = &event.payload {
        if let Some(owner) = value["record"]["address"]
            .as_str()
            .or_else(|| value["agent"].as_str())
        {
            return Some(owner);
        }
    }
    super::conversation_event_turn(&event.payload)
        .and_then(|turn| turn_owners.get(&turn))
        .map(String::as_str)
}

pub(super) fn is_created_file_write(event: &StoredEvent) -> bool {
    matches!(
        &event.payload,
        EventPayload::ToolResult {
            structured: Some(value),
            ..
        } if value["kind"] == "file_write" && value["created"] == true
    )
}

pub(super) fn file_state_at(
    events: &[StoredEvent],
    target_path: &Path,
    workspace: &Path,
) -> FileStateAt {
    for event in events.iter().rev() {
        let EventPayload::ToolResult {
            structured: Some(value),
            ..
        } = &event.payload
        else {
            continue;
        };
        let Some(path) = value["canonical_path"]
            .as_str()
            .or_else(|| value["path"].as_str())
        else {
            continue;
        };
        if !Path::new(path).is_absolute() {
            continue;
        }
        let path_matches = resolve_revert_path(workspace, path).as_deref() == Some(target_path);
        if !path_matches {
            continue;
        }
        if value["kind"] == "file_content"
            && value["cached"] == false
            && value["is_full_file_snapshot"].as_bool().unwrap_or(false)
        {
            if let Some(content) = value["raw_text"].as_str() {
                return FileStateAt::Exists(content.to_owned());
            }
        }
        if REVERTABLE_KINDS.contains(&value["kind"].as_str().unwrap_or_default()) {
            if let Some(content) = value["raw_text_after"].as_str() {
                return FileStateAt::Exists(content.to_owned());
            }
        }
    }
    FileStateAt::Unrecoverable
}
