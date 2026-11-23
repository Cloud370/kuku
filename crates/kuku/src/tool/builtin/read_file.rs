use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::conversation::address::ConversationAddress;
use crate::event::StoredEvent;
use crate::tool::ToolResultEnvelope;

use super::common::{
    file_read_error, find_covering_read, optional_positive_usize, requested_line_count,
    required_path, resolve_path, ExecutorObservation, ExecutorObservationSource, FileCancellation,
};

mod scan;

const PAGE_NOTICE_BYTES: usize = 128;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReadBy {
    Model,
    Executor,
}

struct ReadRequest<'a> {
    path: &'a str,
    offset: usize,
    limit: Option<usize>,
}

pub(crate) fn read_file(
    args: &Value,
    workspace: &Path,
    conversation: &ConversationAddress,
    prior_events: &[StoredEvent],
    read_event_id: u64,
    limits: &crate::RuntimeLimits,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    read_file_impl(
        args,
        workspace,
        ReadBy::Model,
        limits.max_preview_bytes - PAGE_NOTICE_BYTES,
        limits.workspace_io.scan_bytes,
        Some((prior_events, conversation, read_event_id)),
        cancelled,
    )
    .0
}

pub(crate) fn read_file_for_executor(
    args: &Value,
    workspace: &Path,
    max_bytes: usize,
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> (ToolResultEnvelope, Option<ExecutorObservation>) {
    read_file_impl(
        args,
        workspace,
        ReadBy::Executor,
        max_bytes,
        scan_bytes,
        None,
        cancelled,
    )
}

fn read_file_impl(
    args: &Value,
    workspace: &Path,
    read_by: ReadBy,
    max_bytes: usize,
    scan_bytes: u64,
    model_history: Option<(&[StoredEvent], &ConversationAddress, u64)>,
    cancelled: &FileCancellation<'_>,
) -> (ToolResultEnvelope, Option<ExecutorObservation>) {
    let request = match read_request(args) {
        Ok(request) => request,
        Err(result) => return (result, None),
    };
    let resolved = match resolve_path(workspace, request.path) {
        Ok(resolved) => resolved,
        Err(result) => return (result, None),
    };
    let mut view = match scan::scan(
        &resolved.path,
        &request,
        read_by,
        max_bytes,
        scan_bytes,
        cancelled,
    ) {
        Ok(view) => view,
        Err(error) => {
            return (file_read_error(Path::new(&resolved.relative), error), None);
        }
    };
    let requested = requested_line_count(request.offset, request.limit, view.total_lines);
    let prior = model_history
        .filter(|_| requested != 0)
        .and_then(|(events, conversation, _)| {
            find_covering_read(
                events,
                conversation,
                &resolved.path,
                &view.content_hash,
                request.offset,
                requested,
            )
        });
    let summary = prior.as_ref().map_or_else(
        || {
            read_summary(
                &resolved.relative,
                request.offset,
                view.line_count,
                view.total_lines,
                view.truncated,
            )
        },
        |read| {
            format!(
                "already read {}; unchanged since event {}",
                resolved.relative, read.event_id
            )
        },
    );
    let is_full_file_snapshot =
        request.offset == 1 && view.line_count == view.total_lines && !view.truncated;
    let end_byte = view.start_byte + view.raw_text.len();
    let sampled_line = if matches!(read_by, ReadBy::Model) && view.truncated && view.line_count == 0
    {
        Some(request.offset)
    } else {
        None
    };
    let next_offset = request
        .offset
        .saturating_add(view.line_count.max(usize::from(sampled_line.is_some())));
    let next_offset =
        (next_offset > request.offset && next_offset <= view.total_lines).then_some(next_offset);
    let mut structured = serde_json::json!({
        "kind": "file_content",
        "path": resolved.relative,
        "canonical_path": resolved.path.to_string_lossy(),
        "content_hash": view.content_hash,
        "raw_text": view.raw_text,
        "size_bytes": view.size_bytes,
        "read_by": read_by,
        "start_byte": view.start_byte,
        "end_byte": end_byte,
        "start_line": request.offset,
        "line_count": view.line_count,
        "total_lines": view.total_lines,
        "next_offset": next_offset,
        "sampled_line": sampled_line,
        "line_numbered": matches!(read_by, ReadBy::Model),
        "is_full_file_snapshot": is_full_file_snapshot,
        "cached": prior.is_some(),
    });
    if let Some((_, _, event_id)) = model_history {
        structured["read_event_id"] = event_id.into();
    }
    if let Some(prior) = prior {
        structured["prior_read_event_id"] = prior.event_id.into();
    }
    let observation = if matches!(read_by, ReadBy::Executor) {
        view.model_content = summary.clone();
        Some(ExecutorObservation {
            canonical_path: resolved.path,
            content_hash: view.content_hash,
            range: view.start_byte..end_byte,
            size_bytes: view.size_bytes,
            complete: is_full_file_snapshot,
            source: ExecutorObservationSource::Read,
        })
    } else {
        if view.truncated {
            use std::fmt::Write;
            if let Some(line) = sampled_line {
                let _ = write!(view.model_content, "\n[Line {line} exceeds the preview limit. This is a prefix. To extract locally, try run_js tools.read_file with offset={line}, limit=1. Check truncated.");
            } else {
                let end_line = request.offset + view.line_count - 1;
                let _ = write!(
                    view.model_content,
                    "\n[Showing lines {}-{end_line} of {}.",
                    request.offset, view.total_lines
                );
            }
            if let Some(next) = next_offset {
                let _ = write!(view.model_content, " Continue with offset={next}.");
            }
            view.model_content.push(']');
        }
        None
    };
    let result = if view.truncated {
        ToolResultEnvelope::ok_truncated(summary, view.model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, view.model_content, structured)
    };
    (result, observation)
}

fn read_request(args: &Value) -> Result<ReadRequest<'_>, ToolResultEnvelope> {
    Ok(ReadRequest {
        path: required_path("read_file", args)?,
        offset: optional_positive_usize(args, "offset")?.unwrap_or(1),
        limit: optional_positive_usize(args, "limit")?,
    })
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
        if truncated {
            return format!("read {path}, line {offset} exceeds the result byte limit");
        }
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

    const TEST_PREVIEW_BYTES: usize = 1024;

    fn read_file(
        args: &Value,
        workspace: &Path,
        prior_events: &[StoredEvent],
        read_event_id: u64,
    ) -> ToolResultEnvelope {
        super::read_file(
            args,
            workspace,
            &crate::conversation::address::ConversationAddress::MAIN,
            prior_events,
            read_event_id,
            &test_limits(),
            &|| false,
        )
    }

    fn test_limits() -> crate::RuntimeLimits {
        crate::RuntimeLimits {
            max_preview_bytes: TEST_PREVIEW_BYTES,
            ..crate::RuntimeLimits::default()
        }
    }

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
            content,
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
            "1\tfirst\n2\tsecond\n3\tthird",
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
            "1\t# Project",
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

    #[test]
    fn truncated_read_snapshot_ends_at_last_fully_rendered_line() {
        let dir = workspace();
        let visible_line = "a".repeat(TEST_PREVIEW_BYTES - PAGE_NOTICE_BYTES - 2);
        let content = format!("{visible_line}\nhidden tail\n");
        std::fs::write(dir.path().join("README.md"), &content).unwrap();

        let result = read_file(
            &serde_json::json!({"path": "README.md"}),
            dir.path(),
            &[],
            17,
        );

        assert!(result.truncated);
        assert!(result.model_content.len() <= TEST_PREVIEW_BYTES);
        assert!(result.model_content.ends_with("Continue with offset=2.]"));
        let structured = result.structured.unwrap();
        assert_eq!(structured["raw_text"], format!("{visible_line}\n"));
        assert_eq!(structured["line_count"], 1);
        assert_eq!(structured["total_lines"], 2);
        assert_eq!(structured["is_full_file_snapshot"], false);
        assert_eq!(2, structured["next_offset"]);
        assert_eq!(visible_line.len() + 1, structured["end_byte"]);

        let next = read_file(
            &serde_json::json!({"path": "README.md", "offset": structured["next_offset"], "limit": 100}),
            dir.path(),
            &[],
            18,
        );
        assert!(!next.truncated);
        assert_eq!("2\thidden tail", next.model_content);
        let next = next.structured.unwrap();
        assert_eq!(structured["end_byte"], next["start_byte"]);
        assert_eq!(content.len(), next["end_byte"]);
        assert!(next["next_offset"].is_null());
    }

    #[test]
    fn oversized_utf8_line_is_sampled_without_granting_file_evidence() {
        let dir = workspace();
        let line = "文".repeat(30_000);
        let content = format!("{line}\r\nnext\r\n");
        std::fs::write(dir.path().join("sample.txt"), &content).unwrap();
        let args = serde_json::json!({"path": "sample.txt"});
        let result = read_file(&args, dir.path(), &[], 1);

        assert!(result.truncated);
        assert!(result.model_content.len() <= TEST_PREVIEW_BYTES);
        assert!(result
            .model_content
            .starts_with(&format!("1\t{}\n", "文".repeat(85))));
        assert!(result.model_content.ends_with("Continue with offset=2.]"));
        let facts = result.structured.unwrap();
        assert_eq!(1, facts["sampled_line"]);
        assert_eq!(2, facts["next_offset"]);
        assert_eq!(0, facts["line_count"]);
        assert_eq!(facts["start_byte"], facts["end_byte"]);
        assert_eq!("", facts["raw_text"]);
        assert_eq!(false, facts["is_full_file_snapshot"]);

        let args = serde_json::json!({"path": "sample.txt", "offset": 1, "limit": 1});
        let (local, observation) =
            read_file_for_executor(&args, dir.path(), content.len(), u64::MAX, &|| false);
        assert!(!local.truncated);
        let observation = observation.unwrap();
        assert_eq!(0..line.len() + 2, observation.range);
        assert_eq!(format!("{line}\r\n"), local.structured.unwrap()["raw_text"]);
    }

    #[test]
    fn whole_file_writes_require_same_version_pages_without_gaps() {
        let dir = workspace();
        let path = dir.path().join("sample.txt");
        let first = "a".repeat(TEST_PREVIEW_BYTES - PAGE_NOTICE_BYTES - 2);
        let original = format!("{first}\r\nold tail\r\n");
        let changed = format!("{first}\r\nnew tail\r\n");
        let page = |offset, id| {
            let args = serde_json::json!({"path":"sample.txt", "offset":offset});
            let result = read_file(&args, dir.path(), &[], id);
            stored_read_event(id, &result.model_content, result.structured.unwrap())
        };
        let write = |events: &[StoredEvent]| {
            super::super::write_file(
                &serde_json::json!({"path":"sample.txt", "content":"replacement\n"}),
                dir.path(),
                &ConversationAddress::MAIN,
                events,
                crate::WorkspaceIoLimits::default(),
                &|| false,
            )
        };
        std::fs::write(&path, original).unwrap();
        let old_head = page(1, 1);
        std::fs::write(&path, &changed).unwrap();
        let tail = page(2, 2);
        assert_eq!("error", write(&[old_head, tail.clone()]).status);
        assert_eq!("error", write(std::slice::from_ref(&tail)).status);

        let head = page(1, 3);
        let rewind = StoredEvent {
            id: 4,
            payload: crate::event::EventPayload::ConversationRollback {
                ts: "ts".into(),
                conversation: "main".into(),
                to_turn: 1,
                to_event_id: 3,
                scope: crate::event::RollbackScope::ConversationOnly,
            },
        };
        assert_eq!("error", write(&[tail.clone(), head.clone(), rewind]).status);
        assert_eq!(changed, std::fs::read_to_string(&path).unwrap());
        assert_eq!("ok", write(&[tail, head]).status);
        assert_eq!("replacement\n", std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn executor_read_preserves_raw_lines_and_offsets_across_utf8_buffer_boundaries() {
        let dir = workspace();
        let prefix = format!("{}中\n", "x".repeat(65_535));
        let content = format!("{prefix}第二行\r\n第三行\n最后");
        std::fs::write(dir.path().join("README.md"), &content).unwrap();

        let (result, observation) = read_file_for_executor(
            &serde_json::json!({"path":"README.md","offset":2}),
            dir.path(),
            "第二行\r\n第三行\n".len(),
            u64::MAX,
            &|| false,
        );

        assert!(result.truncated);
        let read = observation.unwrap();
        let raw = "第二行\r\n第三行\n";
        assert_eq!(prefix.len()..prefix.len() + raw.len(), read.range);
        assert_eq!(content_hash(content.as_bytes()), read.content_hash);
        assert_eq!(content.len(), read.size_bytes);
        assert!(!read.complete);
        let data = result.structured.unwrap();
        assert_eq!(raw, data["raw_text"]);
        assert_eq!(2, data["line_count"]);
        assert_eq!(4, data["total_lines"]);
        assert_eq!("executor", data["read_by"]);
        assert_eq!(false, data["line_numbered"]);
    }

    #[test]
    fn executor_read_rejects_invalid_utf8_outside_the_requested_lines() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), b"first\n\xff").unwrap();

        let (result, observation) = read_file_for_executor(
            &serde_json::json!({"path":"README.md","limit":1}),
            dir.path(),
            80,
            u64::MAX,
            &|| false,
        );

        assert_eq!("error", result.status);
        assert!(observation.is_none());
        assert!(result.model_content.contains("not valid UTF-8"));
    }

    #[test]
    fn read_cache_is_scoped_to_active_conversation() {
        let dir = workspace();
        let content = "first\nsecond\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let mut review_read = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content.as_bytes(),
            true,
            content,
            "1\tfirst\n2\tsecond",
        );
        if let crate::event::EventPayload::ToolResult { conversation, .. } =
            &mut review_read.payload
        {
            *conversation = Some("review".to_string());
        }

        let main_result = super::read_file(
            &serde_json::json!({"path": "README.md"}),
            dir.path(),
            &crate::conversation::address::ConversationAddress::MAIN,
            std::slice::from_ref(&review_read),
            18,
            &test_limits(),
            &|| false,
        );
        let review_result = super::read_file(
            &serde_json::json!({"path": "README.md"}),
            dir.path(),
            &crate::conversation::address::ConversationAddress::parse("review").unwrap(),
            &[review_read],
            19,
            &test_limits(),
            &|| false,
        );

        assert_eq!(main_result.structured.unwrap()["cached"], false);
        assert_eq!(review_result.structured.unwrap()["cached"], true);
    }
}
