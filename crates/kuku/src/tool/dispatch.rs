use std::path::Path;

use serde_json::Value;

use crate::tool::{builtin, ToolResultEnvelope};

pub(crate) fn dispatch(name: &str, args: &Value, workspace: &Path) -> ToolResultEnvelope {
    match name {
        "find_files" => builtin::find_files(args, workspace),
        "edit_file" | "write_file" | "run_command" => ToolResultEnvelope::blocked(
            format!("blocked by permission: {name} requires a permission gate"),
            format!("{name} was not executed because the permission gate is not available yet"),
        ),
        "read_file" | "search_text" => ToolResultEnvelope::error(
            format!("failed: {name} not implemented"),
            format!("{name} is declared but not implemented in this build stage"),
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
    fn dispatches_find_files_and_rejects_unsupported_tools() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "content").unwrap();

        let found = dispatch("find_files", &serde_json::json!({"path": "."}), dir.path());
        assert_eq!(found.status, "ok");
        assert_eq!(found.model_content, "a.txt");

        let unsupported = dispatch("read_file", &serde_json::json!({"path": "a.txt"}), dir.path());
        assert_eq!(unsupported.status, "error");
        assert!(unsupported.model_content.contains("not implemented"));

        let gated = dispatch("run_command", &serde_json::json!({"command": "ls", "timeout": 1}), dir.path());
        assert_eq!(gated.status, "blocked");
        assert!(gated.model_content.contains("permission gate"));
    }
}
