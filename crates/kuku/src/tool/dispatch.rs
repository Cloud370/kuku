use std::path::Path;

use serde_json::Value;

use crate::event::StoredEvent;
use crate::tool::{builtin, ToolResultEnvelope};

pub(crate) fn dispatch(
    name: &str,
    args: &Value,
    workspace: &Path,
    prior_events: &[StoredEvent],
    result_event_id: u64,
) -> ToolResultEnvelope {
    match name {
        "find_files" => builtin::find_files(args, workspace),
        "read_file" => builtin::read_file(args, workspace, prior_events, result_event_id),
        "search_text" => builtin::search_text(args, workspace),
        "edit_file" | "write_file" | "run_command" => ToolResultEnvelope::blocked(
            format!("blocked by permission: {name} requires a permission gate"),
            format!("{name} was not executed because the permission gate is not available yet"),
        ),
        _ => ToolResultEnvelope::error(
            format!("failed: unknown tool: {name}"),
            format!("unknown tool: {name}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_read_tools_and_rejects_gated_or_unknown_tools() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\ncontent\n").unwrap();

        let found = dispatch("find_files", &serde_json::json!({"path": "."}), dir.path(), &[], 1);
        assert_eq!(found.status, "ok");
        assert_eq!(found.model_content, "a.txt");

        let read = dispatch("read_file", &serde_json::json!({"path": "a.txt"}), dir.path(), &[], 2);
        assert_eq!(read.status, "ok");
        assert!(read.model_content.contains("1\tneedle"));
        assert_eq!(read.structured.as_ref().unwrap()["read_event_id"], 2);

        let searched = dispatch(
            "search_text",
            &serde_json::json!({"pattern": "needle", "view": "lines"}),
            dir.path(),
            &[],
            3,
        );
        assert_eq!(searched.status, "ok");
        assert_eq!(searched.model_content, "a.txt:1: needle");

        let gated = dispatch(
            "run_command",
            &serde_json::json!({"command": "ls", "timeout": 1}),
            dir.path(),
            &[],
            4,
        );
        assert_eq!(gated.status, "blocked");
        assert!(gated.model_content.contains("permission gate"));

        let unknown = dispatch("missing_tool", &serde_json::json!({}), dir.path(), &[], 5);
        assert_eq!(unknown.status, "error");
        assert!(unknown.model_content.contains("unknown tool"));
    }
}
