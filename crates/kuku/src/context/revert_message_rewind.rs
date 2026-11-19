use std::path::Path;

use crate::event::{EventPayload, StoredEvent};

use super::{resolve_revert_path, FileStateAt, REVERTABLE_KINDS};

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
        let path_matches = ["canonical_path", "path"].into_iter().any(|key| {
            value[key]
                .as_str()
                .and_then(|path| resolve_revert_path(workspace, path))
                .as_deref()
                == Some(target_path)
        });
        if !path_matches {
            continue;
        }
        if value["kind"] == "file_content"
            && value["cached"] == false
            && value["is_full_file_snapshot"].as_bool().unwrap_or(false)
        {
            if let Some(content) = value["raw_text_after"].as_str() {
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
