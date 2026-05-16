use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::event::StoredEvent;
use crate::tool::ToolResultEnvelope;

use super::common::{
    content_hash, find_covering_read, join_bounded_strings, optional_positive_usize,
    requested_line_count, resolve_path,
};

const READ_FILE_MAX_CHARS: usize = 80_000;

struct ReadRequest {
    path: String,
    offset: usize,
    limit: Option<usize>,
}

pub(crate) fn read_file(
    args: &Value,
    workspace: &Path,
    prior_events: &[StoredEvent],
    read_event_id: u64,
) -> ToolResultEnvelope {
    let request = match read_request(args) {
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

    let hash = content_hash(&bytes);
    let lines = content.split_inclusive('\n').collect::<Vec<_>>();
    let total_lines = lines.len();
    let requested_line_count = requested_line_count(request.offset, request.limit, total_lines);

    let start_index = request.offset.saturating_sub(1).min(total_lines);
    let end_index = request.limit.map_or(total_lines, |limit| {
        start_index.saturating_add(limit).min(total_lines)
    });
    let (raw_text, model_content, line_count, truncated) =
        render_read_file_view(&lines, start_index, end_index);
    let is_full_file_snapshot = request.offset == 1 && line_count == total_lines && !truncated;

    if requested_line_count > 0 {
        if let Some(prior) = find_covering_read(
            prior_events,
            &resolved.path,
            &hash,
            request.offset,
            requested_line_count,
        ) {
            let summary = format!(
                "already read {}; unchanged since event {}",
                resolved.relative, prior.event_id
            );
            let structured = serde_json::json!({
                "kind": "file_content",
                "path": resolved.relative,
                "canonical_path": resolved.path.to_string_lossy(),
                "content_hash": hash,
                "raw_text": raw_text,
                "size_bytes": bytes.len(),
                "read_event_id": read_event_id,
                "prior_read_event_id": prior.event_id,
                "start_line": request.offset,
                "line_count": requested_line_count,
                "total_lines": total_lines,
                "line_numbered": true,
                "is_full_file_snapshot": is_full_file_snapshot,
                "cached": true,
            });
            return ToolResultEnvelope::ok(summary, model_content, structured);
        }
    }

    let summary = read_summary(
        &resolved.relative,
        request.offset,
        line_count,
        total_lines,
        truncated,
    );
    let structured = serde_json::json!({
        "kind": "file_content",
        "path": resolved.relative,
        "canonical_path": resolved.path.to_string_lossy(),
        "content_hash": hash,
        "raw_text": raw_text,
        "size_bytes": bytes.len(),
        "read_event_id": read_event_id,
        "start_line": request.offset,
        "line_count": line_count,
        "total_lines": total_lines,
        "line_numbered": true,
        "is_full_file_snapshot": is_full_file_snapshot,
        "cached": false,
    });

    if truncated {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

fn read_request(args: &Value) -> Result<ReadRequest, ToolResultEnvelope> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing path",
            "read_file requires path",
        ));
    };
    Ok(ReadRequest {
        path: path.to_string(),
        offset: optional_positive_usize(args, "offset")?.unwrap_or(1),
        limit: optional_positive_usize(args, "limit")?,
    })
}

fn render_read_file_view(
    lines: &[&str],
    start_index: usize,
    end_index: usize,
) -> (String, String, usize, bool) {
    let raw_text = lines[start_index..end_index].concat();
    let mut rendered = Vec::new();
    for (index, line) in lines[start_index..end_index].iter().enumerate() {
        let line_number = start_index + index + 1;
        rendered.push(format!(
            "{line_number}\t{}",
            line.trim_end_matches(['\r', '\n'])
        ));
    }
    let (model_content, truncated) = join_bounded_strings(
        &rendered,
        READ_FILE_MAX_CHARS,
        "(Results are truncated. Use offset and limit to read a smaller range.)",
    );
    let line_count = if truncated {
        model_content
            .lines()
            .filter(|line| !line.starts_with("(Results are truncated."))
            .count()
    } else {
        rendered.len()
    };
    (raw_text, model_content, line_count, truncated)
}

fn read_summary(
    path: &str,
    offset: usize,
    line_count: usize,
    total_lines: usize,
    truncated: bool,
) -> String {
    if total_lines == 0 {
        return format!("read {path}, 0 lines of 0");
    }
    if line_count == 0 {
        return format!("read {path}, no lines at offset {offset} of {total_lines}");
    }
    let end_line = offset + line_count - 1;
    if truncated {
        format!("read {path}, lines {offset}-{end_line} of {total_lines}, results truncated")
    } else {
        format!("read {path}, lines {offset}-{end_line} of {total_lines}")
    }
}

#[cfg(test)]
mod tests {
    use super::super::common::content_hash;
    use super::super::test_helpers::{read_snapshot_event, stored_read_event, workspace};
    use super::*;

    #[test]
    fn read_file_returns_line_numbered_content_and_snapshot_metadata() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "first\nsecond\nthird\n").unwrap();

        let result = read_file(
            &serde_json::json!({"path": "README.md", "offset": 2, "limit": 2}),
            dir.path(),
            &[],
            17,
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.summary, "read README.md, lines 2-3 of 3");
        assert_eq!(result.model_content, "2\tsecond\n3\tthird");
        let structured = result.structured.unwrap();
        assert_eq!(structured["kind"], "file_content");
        assert_eq!(structured["path"], "README.md");
        assert!(structured["canonical_path"]
            .as_str()
            .unwrap()
            .ends_with("README.md"));
        assert!(structured["content_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(structured["read_event_id"], 17);
        assert_eq!(structured["start_line"], 2);
        assert_eq!(structured["line_count"], 2);
        assert_eq!(structured["total_lines"], 3);
        assert_eq!(structured["line_numbered"], true);
        assert_eq!(structured["is_full_file_snapshot"], false);
        assert_eq!(structured["cached"], false);
    }

    #[test]
    fn read_file_preserves_canonical_raw_text_for_fresh_and_cached_reads() {
        let dir = workspace();
        let content = "first\nsecond\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();

        let fresh = read_file(
            &serde_json::json!({"path": "README.md"}),
            dir.path(),
            &[],
            17,
        );

        assert_eq!(fresh.status, "ok");
        assert_eq!(fresh.summary, "read README.md, lines 1-2 of 2");
        assert_eq!(fresh.model_content, "1\tfirst\n2\tsecond");
        let fresh_structured = fresh.structured.unwrap();
        assert_eq!(fresh_structured["raw_text"], content);
        assert_eq!(fresh_structured["cached"], false);
        assert_eq!(fresh_structured["is_full_file_snapshot"], true);

        let prior = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content.as_bytes(),
            true,
            "1\tfirst\n2\tsecond",
        );
        let cached = read_file(
            &serde_json::json!({"path": "README.md"}),
            dir.path(),
            &[prior],
            18,
        );

        assert_eq!(cached.status, "ok");
        assert_eq!(
            cached.summary,
            "already read README.md; unchanged since event 17"
        );
        assert_eq!(cached.model_content, "1\tfirst\n2\tsecond");
        let cached_structured = cached.structured.unwrap();
        assert_eq!(cached_structured["raw_text"], content);
        assert_eq!(cached_structured["cached"], true);
        assert_eq!(cached_structured["prior_read_event_id"], 17);
    }

    #[test]
    fn read_file_uses_event_derived_cache_for_unchanged_covered_reads() {
        let dir = workspace();
        let content = b"first\nsecond\nthird\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let canonical = dir.path().join("README.md").canonicalize().unwrap();
        let prior = stored_read_event(
            17,
            serde_json::json!({
                "kind": "file_content",
                "path": "README.md",
                "canonical_path": canonical.to_string_lossy(),
                "content_hash": content_hash(content),
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 3,
                "total_lines": 3,
                "is_full_file_snapshot": true,
                "cached": false,
            }),
        );

        let result = read_file(
            &serde_json::json!({"path": "README.md", "offset": 2, "limit": 1}),
            dir.path(),
            &[prior],
            21,
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            result.summary,
            "already read README.md; unchanged since event 17"
        );
        assert_eq!(result.model_content, "2\tsecond");
        let structured = result.structured.unwrap();
        assert_eq!(structured["cached"], true);
        assert_eq!(structured["raw_text"], "second\n");
        assert_eq!(structured["prior_read_event_id"], 17);
        assert_eq!(structured["read_event_id"], 21);
    }

    #[test]
    fn read_file_blocks_sensitive_paths_and_rejects_invalid_pagination() {
        let dir = workspace();
        std::fs::write(dir.path().join(".env"), "TOKEN=secret").unwrap();
        std::fs::write(dir.path().join("credentials.json"), "{}").unwrap();

        let blocked_env = read_file(&serde_json::json!({"path": ".env"}), dir.path(), &[], 1);
        assert_eq!(blocked_env.status, "blocked");

        let blocked_credentials = read_file(
            &serde_json::json!({"path": "credentials.json"}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(blocked_credentials.status, "blocked");

        let invalid_offset = read_file(
            &serde_json::json!({"path": "README.md", "offset": 0}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(invalid_offset.status, "error");
        assert!(invalid_offset.model_content.contains("offset must be >= 1"));

        let invalid_limit = read_file(
            &serde_json::json!({"path": "README.md", "limit": 0}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(invalid_limit.status, "error");
        assert!(invalid_limit.model_content.contains("limit must be >= 1"));
    }

    #[test]
    fn read_file_reports_empty_and_eof_ranges_explicitly() {
        let dir = workspace();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();

        let empty = read_file(
            &serde_json::json!({"path": "empty.txt"}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(empty.summary, "read empty.txt, 0 lines of 0");
        assert_eq!(empty.model_content, "");

        let canonical = dir.path().join("README.md").canonicalize().unwrap();
        let prior = stored_read_event(
            17,
            serde_json::json!({
                "kind": "file_content",
                "path": "README.md",
                "canonical_path": canonical.to_string_lossy(),
                "content_hash": content_hash(b"# Project"),
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 1,
                "total_lines": 1,
                "is_full_file_snapshot": true,
                "cached": false,
            }),
        );
        let eof = read_file(
            &serde_json::json!({"path": "README.md", "offset": 99}),
            dir.path(),
            &[prior],
            2,
        );
        assert_eq!(eof.summary, "read README.md, no lines at offset 99 of 1");
        assert_eq!(eof.model_content, "");
        assert_eq!(eof.structured.unwrap()["cached"], false);
    }
}
