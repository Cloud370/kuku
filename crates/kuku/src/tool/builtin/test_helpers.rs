use std::path::Path;

use crate::event::{EventPayload, StoredEvent};

use super::common::content_hash;

pub(crate) fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join("README.md"), "# Project").unwrap();
    std::fs::write(dir.path().join("docs/tools.md"), "# Tools").unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join(".git/config"), "[core]").unwrap();
    dir
}

pub(crate) fn stored_read_event(id: u64, structured: serde_json::Value) -> StoredEvent {
    StoredEvent {
        id,
        payload: EventPayload::ToolResult {
            turn: 1,
            ts: "2026-05-14T00:00:00Z".to_string(),
            conversation: None,
            tool_call_id: format!("tool_{id}"),
            status: "ok".to_string(),
            summary: "read".to_string(),
            model_content: "content".to_string(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: Some(structured),
        },
    }
}

pub(crate) fn read_snapshot_event(
    id: u64,
    dir: &Path,
    path: &str,
    content: &[u8],
    full: bool,
    model_content: &str,
) -> StoredEvent {
    let canonical = dir.join(path).canonicalize().unwrap();
    StoredEvent {
        id,
        payload: EventPayload::ToolResult {
            turn: 1,
            ts: "2026-05-14T00:00:00Z".to_string(),
            conversation: None,
            tool_call_id: format!("tool_{id}"),
            status: "ok".to_string(),
            summary: "read".to_string(),
            model_content: model_content.to_string(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: Some(serde_json::json!({
                "kind": "file_content",
                "path": path,
                "canonical_path": canonical.to_string_lossy(),
                "content_hash": content_hash(content),
                "read_event_id": id,
                "start_line": 1,
                "line_count": if full {
                    String::from_utf8_lossy(content).lines().count()
                } else {
                    1
                },
                "total_lines": String::from_utf8_lossy(content).lines().count(),
                "is_full_file_snapshot": full,
                "cached": false,
            })),
        },
    }
}
