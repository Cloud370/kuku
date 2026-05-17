use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::event::StoredEvent;
use crate::tool::ToolResultEnvelope;

use super::common::{content_hash, find_write_snapshot, plural, resolve_path, write_atomically};

struct EditRequest {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: bool,
    _brief: String,
}

pub(crate) fn edit_file(
    args: &Value,
    workspace: &Path,
    prior_events: &[StoredEvent],
) -> ToolResultEnvelope {
    let request = match edit_file_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let resolved = match resolve_path(workspace, &request.path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    if !resolved.path.is_file() {
        return ToolResultEnvelope::error(
            format!("failed: not a file: {}", request.path),
            format!("path is not a file: {}", request.path),
        );
    }

    let bytes = match fs::read(&resolved.path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: {error}"),
                format!("error reading file: {}", request.path),
            )
        }
    };
    let content = match String::from_utf8(bytes.clone()) {
        Ok(content) => content,
        Err(_) => {
            return ToolResultEnvelope::error(
                format!("failed: file is not valid UTF-8: {}", resolved.relative),
                format!("file is not valid UTF-8: {}", resolved.relative),
            )
        }
    };
    let current_hash = content_hash(&bytes);
    let Some(snapshot) =
        find_write_snapshot(prior_events, &resolved.path, false, Some(&request.old_text))
    else {
        return ToolResultEnvelope::error(
            format!("failed: read {} before editing", resolved.relative),
            format!(
                "edit_file requires a prior successful read_file snapshot for {}",
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
                "file changed since it was read; read {} again before editing",
                resolved.relative
            ),
        );
    }

    let replacement_count = content.matches(&request.old_text).count();
    if replacement_count == 0 {
        return ToolResultEnvelope::error(
            format!("failed: old_text not found in {}", resolved.relative),
            "old_text was not found".to_string(),
        );
    }
    if replacement_count > 1 && !request.replace_all {
        return ToolResultEnvelope::error(
            format!(
                "failed: old_text matched {replacement_count} times in {}",
                resolved.relative
            ),
            "old_text is not unique; provide more context or set replace_all=true".to_string(),
        );
    }

    let edited = if request.replace_all {
        content.replace(&request.old_text, &request.new_text)
    } else {
        content.replacen(&request.old_text, &request.new_text, 1)
    };
    if let Err(error) = write_atomically(&resolved.path, edited.as_bytes()) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error writing file: {}", resolved.relative),
        );
    }

    let raw_text_after = edited;
    let bytes_written = raw_text_after.len();
    let content_hash_after = content_hash(raw_text_after.as_bytes());
    let canonical_path = fs::canonicalize(&resolved.path)
        .unwrap_or_else(|_| resolved.path.clone())
        .to_string_lossy()
        .into_owned();
    let summary = format!(
        "edited {}, {replacement_count} replacement{}",
        resolved.relative,
        plural(replacement_count)
    );
    ToolResultEnvelope::ok(
        summary.clone(),
        summary,
        serde_json::json!({
            "kind": "file_edit",
            "path": resolved.relative,
            "canonical_path": canonical_path,
            "replacement_count": replacement_count,
            "bytes_written": bytes_written,
            "content_hash": content_hash_after,
            "content_hash_after": content_hash_after,
            "raw_text_after": raw_text_after,
        }),
    )
}

fn edit_file_request(args: &Value) -> Result<EditRequest, ToolResultEnvelope> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing path",
            "edit_file requires path",
        ));
    };
    let Some(old_text) = args.get("old_text").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing old_text",
            "edit_file requires old_text",
        ));
    };
    let Some(new_text) = args.get("new_text").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing new_text",
            "edit_file requires new_text",
        ));
    };
    if old_text.is_empty() {
        return Err(ToolResultEnvelope::error(
            "failed: old_text is empty",
            "old_text must not be empty",
        ));
    }
    let Some(brief) = args.get("brief").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing brief",
            "edit_file requires brief",
        ));
    };
    if brief.trim().is_empty() {
        return Err(ToolResultEnvelope::error(
            "failed: brief is empty",
            "brief must not be empty",
        ));
    }
    Ok(EditRequest {
        path: path.to_string(),
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
        replace_all: args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _brief: brief.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{read_snapshot_event, workspace};
    use super::*;

    #[test]
    fn edit_file_requires_prior_read_and_rejects_stale_snapshot() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "alpha\nbeta\n").unwrap();

        let missing = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega", "brief": "rename alpha"}),
            dir.path(),
            &[],
        );
        assert_eq!(missing.status, "error");
        assert!(missing
            .model_content
            .contains("prior successful read_file snapshot"));

        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            b"alpha\nbeta\n",
            true,
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), "changed\nbeta\n").unwrap();
        let stale = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "beta", "new_text": "gamma", "brief": "change beta"}),
            dir.path(),
            &[snapshot],
        );
        assert_eq!(stale.status, "error");
        assert!(stale.model_content.contains("read README.md again"));
    }

    #[test]
    fn edit_file_replaces_unique_text_or_all_matches() {
        let dir = workspace();
        let content = b"alpha\nbeta\nalpha\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            true,
            "1\talpha\n2\tbeta\n3\talpha",
        );

        let ambiguous = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega", "brief": "rename alpha"}),
            dir.path(),
            std::slice::from_ref(&snapshot),
        );
        assert_eq!(ambiguous.status, "error");
        assert!(ambiguous.model_content.contains("not unique"));

        let unique = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "beta", "new_text": "gamma", "brief": "change beta"}),
            dir.path(),
            std::slice::from_ref(&snapshot),
        );
        assert_eq!(unique.status, "ok");
        assert_eq!(unique.structured.as_ref().unwrap()["replacement_count"], 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "alpha\ngamma\nalpha\n"
        );

        let structured = unique.structured.as_ref().unwrap();
        let canonical_path = dir.path().join("README.md").canonicalize().unwrap();
        assert_eq!(
            structured["canonical_path"].as_str().unwrap(),
            canonical_path.to_string_lossy().as_ref()
        );
        assert_eq!(
            structured["raw_text_after"].as_str().unwrap(),
            "alpha\ngamma\nalpha\n"
        );
        assert!(structured["content_hash_after"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));

        let changed = b"alpha\ngamma\nalpha\n";
        let second_snapshot = read_snapshot_event(
            18,
            dir.path(),
            "README.md",
            changed,
            true,
            "1\talpha\n2\tgamma\n3\talpha",
        );
        let all = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega", "replace_all": true, "brief": "replace all alpha"}),
            dir.path(),
            &[second_snapshot],
        );
        assert_eq!(all.status, "ok");
        assert_eq!(all.structured.as_ref().unwrap()["replacement_count"], 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "omega\ngamma\nomega\n"
        );
    }

    #[test]
    fn edit_file_blocks_sensitive_paths() {
        let dir = workspace();
        std::fs::write(dir.path().join(".env"), "TOKEN=secret").unwrap();

        let edit = edit_file(
            &serde_json::json!({"path": ".env", "old_text": "TOKEN", "new_text": "KEY", "brief": "rename token"}),
            dir.path(),
            &[],
        );
        assert_eq!(edit.status, "blocked");
    }
}
