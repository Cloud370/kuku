use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::event::{EventPayload, StoredEvent};

use super::{resolve_revert_path, REVERTABLE_KINDS};

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
    event_owner_with(event, |turn| turn_owners.get(&turn).map(String::as_str))
}

pub(crate) fn event_owner_with<'a>(
    event: &'a StoredEvent,
    turn_owner: impl FnOnce(u64) -> Option<&'a str>,
) -> Option<&'a str> {
    if let Some(owner) = event.payload.agent_address() {
        return Some(owner);
    }
    super::conversation_event_turn(&event.payload).and_then(turn_owner)
}

/// Resolve event ownership using explicit Agent addresses and current Turn ownership.
pub fn event_owners(events: &[StoredEvent]) -> HashMap<u64, String> {
    let turn_owners = event_turn_owners(events);
    events
        .iter()
        .filter_map(|event| {
            event_owner(event, &turn_owners).map(|owner| (event.id, owner.to_owned()))
        })
        .collect()
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

pub(super) fn file_snapshot<'a>(
    event: &'a StoredEvent,
    workspace: &Path,
) -> Option<(PathBuf, &'a str)> {
    let EventPayload::ToolResult {
        structured: Some(value),
        ..
    } = &event.payload
    else {
        return None;
    };
    let kind = value["kind"].as_str()?;
    let content = if kind == "file_content"
        && value["cached"] == false
        && value["is_full_file_snapshot"] == true
    {
        value["raw_text"].as_str()?
    } else if REVERTABLE_KINDS.contains(&kind) {
        value["raw_text_after"].as_str()?
    } else {
        return None;
    };
    let path = value["canonical_path"]
        .as_str()
        .or_else(|| value["path"].as_str())?;
    if !Path::new(path).is_absolute() {
        return None;
    }
    Some((resolve_revert_path(workspace, path)?, content))
}
