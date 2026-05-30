use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::event::StoredEvent;
use crate::tool::ToolResultEnvelope;

use super::common::{
    content_hash, find_write_snapshot, plural, read_file_as_utf8, require_brief,
    resolve_write_path, write_atomically,
};

struct WriteRequest {
    path: String,
    content: String,
    _brief: String,
}

pub(crate) fn write_file(
    args: &Value,
    workspace: &Path,
    prior_events: &[StoredEvent],
) -> ToolResultEnvelope {
    let request = match write_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let resolved = match resolve_write_path(workspace, &request.path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };

    let created = !resolved.path.exists();
    if !created {
        if !resolved.path.is_file() {
            return ToolResultEnvelope::error(
                format!("failed: not a file: {}", request.path),
                format!("path is not a file: {}", request.path),
            );
        }
        let (_content, bytes) = match read_file_as_utf8(&resolved.path) {
            Ok(result) => result,
            Err(err) => return err,
        };
        let current_hash = content_hash(&bytes);
        let Some(snapshot) = find_write_snapshot(prior_events, &resolved.path, true, None) else {
            return ToolResultEnvelope::error(
                format!(
                    "failed: fully read {} before overwriting",
                    resolved.relative
                ),
                format!(
                    "write_file requires a prior full read_file snapshot before overwriting {}",
                    resolved.relative
                ),
            );
        };
        if snapshot.content_hash != current_hash {
            return ToolResultEnvelope::error(
                format!(
                    "failed: {} changed since event {}",
                    resolved.relative, snapshot.event_id
                ),
                format!(
                    "file changed since it was read; read {} again before overwriting",
                    resolved.relative
                ),
            );
        }
    }

    let raw_text_after = request.content;
    let line_count = raw_text_after.lines().count();
    let bytes_written = raw_text_after.len();
    let content_hash_after = content_hash(raw_text_after.as_bytes());

    if let Err(error) = write_atomically(&resolved.path, raw_text_after.as_bytes()) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error writing file: {}", resolved.relative),
        );
    }

    let canonical_path = fs::canonicalize(&resolved.path)
        .unwrap_or_else(|_| resolved.path.clone())
        .to_string_lossy()
        .into_owned();
    let summary = format!(
        "wrote {}, {line_count} line{}",
        resolved.relative,
        plural(line_count)
    );
    ToolResultEnvelope::ok(
        summary.clone(),
        summary,
        serde_json::json!({
            "kind": "file_write",
            "path": resolved.relative,
            "canonical_path": canonical_path,
            "line_count": line_count,
            "bytes_written": bytes_written,
            "content_hash": content_hash_after,
            "content_hash_after": content_hash_after,
            "raw_text_after": raw_text_after,
            "created": created,
        }),
    )
}

fn write_request(args: &Value) -> Result<WriteRequest, ToolResultEnvelope> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing path",
            "write_file requires path",
        ));
    };
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing content",
            "write_file requires content",
        ));
    };
    let brief = require_brief("write_file", args)?;
    Ok(WriteRequest {
        path: path.to_string(),
        content: content.to_string(),
        _brief: brief,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{read_snapshot_event, workspace};
    use super::*;

    #[test]
    fn write_file_creates_new_file_without_prior_read() {
        let dir = workspace();
        let result = write_file(
            &serde_json::json!({"path": "docs/new.md", "content": "hello\nworld\n", "brief": "create new docs page"}),
            dir.path(),
            &[],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.structured.as_ref().unwrap()["created"], true);
        assert_eq!(result.structured.as_ref().unwrap()["line_count"], 2);
        let structured = result.structured.as_ref().unwrap();
        let canonical_path = dir.path().join("docs/new.md").canonicalize().unwrap();
        assert_eq!(
            structured["canonical_path"].as_str().unwrap(),
            canonical_path.to_string_lossy().as_ref()
        );
        assert_eq!(
            structured["raw_text_after"].as_str().unwrap(),
            "hello\nworld\n"
        );
        assert!(structured["content_hash_after"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docs/new.md")).unwrap(),
            "hello\nworld\n"
        );
    }

    #[test]
    fn write_file_normalizes_new_file_paths_before_parent_check() {
        let dir = workspace();
        let result = write_file(
            &serde_json::json!({"path": "missing/../docs/normalized.md", "content": "normalized\n", "brief": "create normalized file"}),
            dir.path(),
            &[],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            result.structured.as_ref().unwrap()["path"],
            "docs/normalized.md"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docs/normalized.md")).unwrap(),
            "normalized\n"
        );
    }

    #[test]
    fn write_file_requires_full_snapshot_for_overwrite_and_rejects_stale_snapshot() {
        let dir = workspace();
        let original = b"alpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let partial = read_snapshot_event(17, dir.path(), "README.md", original, false, "1\talpha");

        let partial_result = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n", "brief": "overwrite readme"}),
            dir.path(),
            &[partial],
        );
        assert_eq!(partial_result.status, "error");
        assert!(partial_result
            .model_content
            .contains("prior full read_file snapshot"));

        let full = read_snapshot_event(
            18,
            dir.path(),
            "README.md",
            original,
            true,
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), "changed\n").unwrap();
        let stale = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n", "brief": "overwrite readme"}),
            dir.path(),
            std::slice::from_ref(&full),
        );
        assert_eq!(stale.status, "error");
        assert!(stale.model_content.contains("read README.md again"));

        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let ok = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n", "brief": "overwrite readme"}),
            dir.path(),
            &[full],
        );
        assert_eq!(ok.status, "ok");
        let structured = ok.structured.as_ref().unwrap();
        let canonical_path = dir.path().join("README.md").canonicalize().unwrap();
        assert_eq!(structured["created"], false);
        assert_eq!(
            structured["canonical_path"].as_str().unwrap(),
            canonical_path.to_string_lossy().as_ref()
        );
        assert_eq!(
            structured["raw_text_after"].as_str().unwrap(),
            "replacement\n"
        );
        assert!(structured["content_hash_after"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "replacement\n"
        );
    }

    #[test]
    fn write_file_blocks_sensitive_paths() {
        let dir = workspace();
        let write = write_file(
            &serde_json::json!({"path": ".env.local", "content": "TOKEN=secret", "brief": "write secret"}),
            dir.path(),
            &[],
        );
        assert_eq!(write.status, "blocked");
    }
}
