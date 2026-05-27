use std::path::Path;

use serde_json::Value;

#[cfg(test)]
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::event::EventPayload;
use crate::event::StoredEvent;
use crate::tool::{builtin, ToolResultEnvelope};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch(
    name: &str,
    args: &Value,
    workspace: &Path,
    kuku_home: &Path,
    prior_events: &[StoredEvent],
    result_event_id: u64,
    tool_call_id: Option<&str>,
    config: &crate::config::Config,
    catalog: &crate::prompt::PromptCatalog,
) -> ToolResultEnvelope {
    match name {
        "agent" => ToolResultEnvelope::error(
            "agent tool must be executed via subagent handler".to_string(),
            "the agent tool can only be invoked through the normal agent loop".to_string(),
        ),
        "find_files" => builtin::find_files(args, workspace),
        "read_file" => builtin::read_file(args, workspace, prior_events, result_event_id),
        "search_text" => builtin::search_text(args, workspace),
        "fetch_url" => builtin::fetch_url(args, workspace).await,
        "fetch_web" => builtin::fetch_web(args, workspace, config, catalog).await,
        "edit_file" | "write_file" | "remember_memory" | "forget_memory" | "run_command"
            if has_denied_permission(prior_events, tool_call_id) =>
        {
            ToolResultEnvelope::blocked(
                format!("blocked by permission: {name} requires a permission gate"),
                format!(
                    "{name} was not executed because the permission gate denied this tool call"
                ),
            )
        }
        "edit_file" => builtin::edit_file(args, workspace, prior_events),
        "write_file" => builtin::write_file(args, workspace, prior_events),
        "remember_memory" => builtin::remember_memory_with_home(args, workspace, kuku_home),
        "forget_memory" => builtin::forget_memory_with_home(args, workspace, kuku_home),
        "run_command" => builtin::run_command(args, workspace, None, None).await,
        _ => ToolResultEnvelope::error(
            format!("failed: unknown tool: {name}"),
            format!("unknown tool: {name}"),
        ),
    }
}

fn has_denied_permission(events: &[StoredEvent], tool_call_id: Option<&str>) -> bool {
    let Some(tool_call_id) = tool_call_id else {
        return false;
    };
    events.iter().any(|event| {
        matches!(
            &event.payload,
            crate::event::EventPayload::PermissionDecision { tool_call_id: id, decision, .. }
                if id == tool_call_id && decision == "deny"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config_and_catalog(
    ) -> (crate::config::Config, crate::prompt::PromptCatalog) {
        let catalog = crate::prompt::catalog::builtin_prompt_catalog();
        let toml_str = crate::config::generate_default();
        let file: crate::config::ConfigFile = toml::from_str(toml_str).unwrap();
        let config = file.resolve().unwrap();
        (config, catalog)
    }

    fn read_event(id: u64, dir: &std::path::Path, path: &str, content: &[u8]) -> StoredEvent {
        let canonical = dir.join(path).canonicalize().unwrap();
        let digest = Sha256::digest(content);
        StoredEvent {
            id,
            payload: EventPayload::ToolResult {
                turn: 1,
                ts: "2026-05-14T00:00:00Z".to_string(),
                tool_call_id: format!("read_{id}"),
                status: "ok".to_string(),
                summary: "read".to_string(),
                model_content: String::from_utf8_lossy(content)
                    .lines()
                    .enumerate()
                    .map(|(index, line)| format!("{}\t{}", index + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n"),
                truncated: false,
                structured: Some(serde_json::json!({
                    "kind": "file_content",
                    "path": path,
                    "canonical_path": canonical.to_string_lossy(),
                    "content_hash": format!("sha256:{digest:x}"),
                    "read_event_id": id,
                    "start_line": 1,
                    "line_count": String::from_utf8_lossy(content).lines().count(),
                    "total_lines": String::from_utf8_lossy(content).lines().count(),
                    "is_full_file_snapshot": true,
                    "cached": false,
                })),
            },
        }
    }

    fn denied_event(tool_call_id: &str) -> StoredEvent {
        StoredEvent {
            id: 99,
            payload: EventPayload::PermissionDecision {
                turn: 1,
                ts: "2026-05-14T00:00:00Z".to_string(),
                tool_call_id: tool_call_id.to_string(),
                decision: "deny".to_string(),
                scope: "once".to_string(),
                source: "runtime".to_string(),
                rule: "permission_gate_unavailable".to_string(),
            },
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatches_read_tools_and_rejects_gated_or_unknown_tools() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\ncontent\n").unwrap();
        let (config, catalog) = test_config_and_catalog();

        let found = dispatch(
            "find_files",
            &serde_json::json!({"path": "."}),
            dir.path(),
            dir.path(),
            &[],
            1,
            None,
            &config,
            &catalog,
        )
        .await;
        assert_eq!(found.status, "ok");
        assert_eq!(found.model_content, "a.txt");

        let read = dispatch(
            "read_file",
            &serde_json::json!({"path": "a.txt"}),
            dir.path(),
            dir.path(),
            &[],
            2,
            None,
            &config,
            &catalog,
        )
        .await;
        assert_eq!(read.status, "ok");
        assert!(read.model_content.contains("1\tneedle"));
        assert_eq!(read.structured.as_ref().unwrap()["read_event_id"], 2);

        let searched = dispatch(
            "search_text",
            &serde_json::json!({"pattern": "needle", "view": "lines"}),
            dir.path(),
            dir.path(),
            &[],
            3,
            None,
            &config,
            &catalog,
        )
        .await;
        assert_eq!(searched.status, "ok");
        assert_eq!(searched.model_content, "a.txt:1: needle");

        let denied = denied_event("tool_command");
        let gated = dispatch(
            "run_command",
            &serde_json::json!({"command": "cargo test", "timeout": 1, "brief": "run tests"}),
            dir.path(),
            dir.path(),
            &[denied],
            4,
            Some("tool_command"),
            &config,
            &catalog,
        )
        .await;
        assert_eq!(gated.status, "blocked");
        assert!(gated.model_content.contains("permission gate denied"));

        let unknown = dispatch(
            "missing_tool",
            &serde_json::json!({}),
            dir.path(),
            dir.path(),
            &[],
            5,
            None,
            &config,
            &catalog,
        )
        .await;
        assert_eq!(unknown.status, "error");
        assert!(unknown.model_content.contains("unknown tool"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_executes_edit_and_write_when_not_denied() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"hello\nworld\n";
        std::fs::write(dir.path().join("a.txt"), original).unwrap();
        let read = read_event(17, dir.path(), "a.txt", original);
        let (config, catalog) = test_config_and_catalog();

        let edited = dispatch(
            "edit_file",
            &serde_json::json!({"path": "a.txt", "old_text": "world", "new_text": "kuku", "brief": "edit a.txt"}),
            dir.path(),
            dir.path(),
            std::slice::from_ref(&read),
            18,
            None,
            &config,
            &catalog,
        )
        .await;
        assert_eq!(edited.status, "ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello\nkuku\n"
        );

        let after_edit = b"hello\nkuku\n";
        let read_after_edit = read_event(19, dir.path(), "a.txt", after_edit);
        let written = dispatch(
            "write_file",
            &serde_json::json!({"path": "a.txt", "content": "done\n", "brief": "write done"}),
            dir.path(),
            dir.path(),
            &[read_after_edit],
            20,
            None,
            &config,
            &catalog,
        )
        .await;
        assert_eq!(written.status, "ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "done\n"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_blocks_edit_write_when_runtime_permission_denied() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"hello\nworld\n";
        std::fs::write(dir.path().join("a.txt"), original).unwrap();
        let read = read_event(17, dir.path(), "a.txt", original);
        let denied = denied_event("tool_edit");
        let (config, catalog) = test_config_and_catalog();

        let result = dispatch(
            "edit_file",
            &serde_json::json!({"path": "a.txt", "old_text": "world", "new_text": "kuku", "brief": "edit a.txt"}),
            dir.path(),
            dir.path(),
            &[read, denied],
            18,
            Some("tool_edit"),
            &config,
            &catalog,
        )
        .await;
        assert_eq!(result.status, "blocked");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello\nworld\n"
        );

        // remember_memory denial
        let memory_denied = denied_event("tool_memory");
        let remember = dispatch(
            "remember_memory",
            &serde_json::json!({"scope": "project", "kind": "how_to_work", "text": "Keep answers concise"}),
            dir.path(),
            dir.path(),
            &[memory_denied],
            19,
            Some("tool_memory"),
            &config,
            &catalog,
        )
        .await;
        assert_eq!(remember.status, "blocked");
        assert!(remember.model_content.contains("permission gate denied"));

        // forget_memory denial
        let memory_denied2 = denied_event("tool_memory");
        let forget = dispatch(
            "forget_memory",
            &serde_json::json!({"scope": "project", "text": "Keep answers concise"}),
            dir.path(),
            dir.path(),
            &[memory_denied2],
            20,
            Some("tool_memory"),
            &config,
            &catalog,
        )
        .await;
        assert_eq!(forget.status, "blocked");
        assert!(forget.model_content.contains("permission gate denied"));
    }

    #[test]
    fn dispatch_uses_the_captured_home_for_memory_tools() {
        let _guard = crate::env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let session_home = tempfile::tempdir().unwrap();
        let runtime_home = tempfile::tempdir().unwrap();
        let workspace = Path::new(dir.path());
        let args = serde_json::json!({"scope": "project", "kind": "how_to_work", "text": "Keep answers concise"});
        let expected_path = crate::session::project_memory_path(
            session_home.path(),
            &std::fs::canonicalize(workspace).unwrap(),
        )
        .unwrap();
        let unexpected_path = crate::session::project_memory_path(
            runtime_home.path(),
            &std::fs::canonicalize(workspace).unwrap(),
        )
        .unwrap();

        let (config, catalog) = test_config_and_catalog();
        let previous = std::env::var_os("KUKU_HOME");
        std::env::set_var("KUKU_HOME", runtime_home.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            dispatch(
                "remember_memory",
                &args,
                workspace,
                session_home.path(),
                &[],
                1,
                None,
                &config,
                &catalog,
            )
            .await
        });
        match previous {
            Some(value) => std::env::set_var("KUKU_HOME", value),
            None => std::env::remove_var("KUKU_HOME"),
        }

        assert_eq!(result.status, "ok");
        assert!(std::fs::read_to_string(&expected_path)
            .unwrap()
            .contains("Keep answers concise"));
        assert!(!unexpected_path.exists());
    }
}
