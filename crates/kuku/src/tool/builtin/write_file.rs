use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::Value;

use crate::WorkspaceIoLimits;
use crate::event::StoredEvent;
use crate::execution::{FileCommitState, FileMutationFacts, FileMutationKind};
use crate::tool::ToolResultEnvelope;

use super::common::{
    ExecutorObservation, FileCancellation, FileScope, IoLimit, WriteEvidence, WriteSnapshotLookup,
    content_hash, expected_version, file_read_error, file_version_with_text, mutation_preview,
    plural, publish_file, require_version, resolve_stat_path, snapshot_stale_error,
};

use super::file_change;

struct WriteRequest<'a> {
    path: &'a str,
    content: &'a str,
}

pub(crate) fn write_file(
    args: &Value,
    scope: FileScope<'_>,
    conversation: &crate::conversation::address::ConversationAddress,
    prior_events: &[StoredEvent],
    limits: WorkspaceIoLimits,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    write_file_with_evidence(
        args,
        scope,
        WriteEvidence::Model {
            conversation,
            events: prior_events,
        },
        limits,
        cancelled,
    )
}

pub(crate) fn write_file_for_executor(
    args: &Value,
    scope: FileScope<'_>,
    reads: &[ExecutorObservation],
    limits: WorkspaceIoLimits,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    write_file_with_evidence(
        args,
        scope,
        WriteEvidence::Executor(reads),
        limits,
        cancelled,
    )
}

fn write_file_with_evidence(
    args: &Value,
    scope: FileScope<'_>,
    evidence: WriteEvidence<'_>,
    limits: WorkspaceIoLimits,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    let request = match write_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let expected = match expected_version(args) {
        Ok(expected) => expected,
        Err(result) => return result,
    };
    let resolved = match scope.resolve(request.path, resolve_stat_path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };

    if request.content.len() > limits.text_bytes {
        let error = std::io::Error::other(IoLimit("text exceeds the file output budget"));
        return file_read_error(Path::new(&resolved.relative), error);
    }
    let text_limit = file_change::MAX_INPUT_BYTES.checked_sub(request.content.len());
    let (before, before_text) =
        match current_version(&resolved.path, limits.scan_bytes, cancelled, text_limit) {
            Ok(before) => before,
            Err(error) => return file_read_error(Path::new(&resolved.relative), error),
        };
    if let Err(result) = require_version(&resolved.relative, expected, before.as_deref()) {
        return result;
    }
    if let Some(current_hash) = &before {
        let snapshot = match evidence.find(&resolved.path, true, None, current_hash) {
            WriteSnapshotLookup::Found(snapshot) => snapshot,
            WriteSnapshotLookup::Rejected(reason) => {
                return ToolResultEnvelope::error_with_reason(
                    format!(
                        "failed: fully read {} before overwriting",
                        resolved.relative
                    ),
                    format!(
                        "write_file requires a prior full read_file snapshot of {} or your own confirmed full write of its current version",
                        resolved.relative
                    ),
                    reason,
                );
            }
        };
        if snapshot.content_hash != *current_hash {
            return snapshot_stale_error(&resolved.relative, snapshot.event_id);
        }
    }

    let bytes = request.content.as_bytes();
    let publication = match publish_file(
        &resolved.path,
        before.as_deref(),
        bytes,
        limits.scan_bytes,
        cancelled,
    ) {
        Ok(publication) => publication,
        Err(error) => return error.result(&resolved.relative),
    };
    let line_count = request.content.lines().count();
    let content_hash_after = content_hash(bytes);
    let mut fields = serde_json::json!({
        "content_hash": content_hash_after,
        "line_count": line_count,
        "created": before.is_none(),
    });
    if let Some(text) = before_text.as_deref() {
        let change = file_change::facts(text, request.content);
        if !change.is_null() {
            fields["change"] = change;
        }
    }
    if evidence.is_model() {
        mutation_preview(request.content, &mut fields);
    }
    let summary = format!(
        "wrote {}, {line_count} line{}",
        resolved.relative,
        plural(line_count)
    );
    publication.result(
        FileMutationFacts {
            kind: FileMutationKind::FileWrite,
            path: resolved.relative,
            canonical_path: resolved.path.to_string_lossy().into_owned(),
            commit_state: FileCommitState::Committed,
            content_hash_before: before,
            content_hash_after: Some(content_hash_after),
            bytes_written: Some(bytes.len()),
            bytes_removed: None,
        },
        summary,
        fields,
    )
}

fn current_version(
    path: &Path,
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
    text_limit: Option<usize>,
) -> std::io::Result<(Option<String>, Option<String>)> {
    match fs::symlink_metadata(path) {
        Ok(_) => file_version_with_text(path, scan_bytes, cancelled, text_limit)
            .map(|(hash, text)| (Some(hash), text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok((None, None)),
        Err(error) => Err(error),
    }
}

fn write_request(args: &Value) -> Result<WriteRequest<'_>, ToolResultEnvelope> {
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
    Ok(WriteRequest { path, content })
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{read_snapshot_event, workspace};
    use super::*;

    fn write_file(
        args: &Value,
        workspace: &Path,
        prior_events: &[StoredEvent],
    ) -> ToolResultEnvelope {
        super::write_file(
            args,
            FileScope::Unprepared(workspace),
            &crate::conversation::address::ConversationAddress::MAIN,
            prior_events,
            crate::WorkspaceIoLimits::default(),
            &|| false,
        )
    }

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
        assert!(
            structured["content_hash_after"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
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
    fn write_file_rejects_an_expected_version_before_creating_parents() {
        let dir = workspace();
        let version = content_hash(b"previous version");
        let result = write_file(
            &serde_json::json!({"path": "new/src/main.rs", "content": "fn main() {}", "expected_version": version}),
            dir.path(),
            &[],
        );

        assert_eq!("error", result.status);
        assert!(!dir.path().join("new").exists());
    }

    #[test]
    fn write_file_requires_full_snapshot_for_overwrite_and_rejects_stale_snapshot() {
        let dir = workspace();
        let original = b"alpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let partial = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            original,
            false,
            "alpha\n",
            "1\talpha",
        );

        let partial_result = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n", "brief": "overwrite readme"}),
            dir.path(),
            &[partial],
        );
        assert_eq!(partial_result.status, "error");
        assert_eq!(
            partial_result.structured.as_ref().unwrap()["reason_code"],
            "full_snapshot_required"
        );
        assert!(
            partial_result
                .model_content
                .contains("prior full read_file snapshot")
        );

        let full = read_snapshot_event(
            18,
            dir.path(),
            "README.md",
            original,
            true,
            "alpha\nbeta\n",
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), "changed\n").unwrap();
        let stale = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n", "brief": "overwrite readme"}),
            dir.path(),
            std::slice::from_ref(&full),
        );
        assert_eq!(stale.status, "error");
        assert_eq!(stale.structured.as_ref().unwrap()["kind"], "error");
        assert_eq!(
            stale.structured.as_ref().unwrap()["reason_code"],
            "snapshot_stale"
        );
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
        assert_eq!(structured["change"]["added_lines"], 1);
        assert_eq!(structured["change"]["removed_lines"], 2);
        assert_eq!(
            structured["change"]["preview_line_numbers"],
            serde_json::json!([1, 2, 1])
        );
        assert_eq!(
            structured["canonical_path"].as_str().unwrap(),
            canonical_path.to_string_lossy().as_ref()
        );
        assert_eq!(
            structured["raw_text_after"].as_str().unwrap(),
            "replacement\n"
        );
        assert!(
            structured["content_hash_after"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
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

    #[test]
    fn executor_can_overwrite_after_receiving_all_pages_of_one_version() {
        let dir = workspace();
        let path = dir.path().join("README.md");
        std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
        let read = |offset, limit| {
            crate::tool::builtin::read_file_for_executor(
                &serde_json::json!({"path":"README.md","offset":offset,"limit":limit}),
                FileScope::Unprepared(dir.path()),
                80,
                u64::MAX,
                &|| false,
            )
            .1
            .unwrap()
        };
        let first = read(1, 1);
        let last = read(3, 1);
        let mut reads = vec![first, last];
        let args = serde_json::json!({"path":"README.md","content":"replacement\n"});

        let partial = write_file_for_executor(
            &args,
            FileScope::Unprepared(dir.path()),
            &reads,
            crate::WorkspaceIoLimits::default(),
            &|| false,
        );
        assert_eq!(
            "full_snapshot_required",
            partial.structured.unwrap()["reason_code"]
        );
        reads.push(read(2, 1));

        let committed = write_file_for_executor(
            &args,
            FileScope::Unprepared(dir.path()),
            &reads,
            crate::WorkspaceIoLimits::default(),
            &|| false,
        );
        assert_eq!("ok", committed.status, "{:?}", committed.structured);
        assert_eq!("replacement\n", std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn pinning_an_old_version_does_not_use_a_newer_read_of_the_same_path() {
        let dir = workspace();
        let path = dir.path().join("README.md");
        let read = || {
            crate::tool::builtin::read_file_for_executor(
                &serde_json::json!({"path":"README.md"}),
                FileScope::Unprepared(dir.path()),
                80,
                u64::MAX,
                &|| false,
            )
            .1
            .unwrap()
        };
        let first = read();
        let args = serde_json::json!({
            "path":"README.md", "content":"replacement", "expected_version":first.content_hash
        });
        std::fs::write(&path, "new version\n").unwrap();
        let newer = read();

        let stale = write_file_for_executor(
            &args,
            FileScope::Unprepared(dir.path()),
            &[first, newer],
            crate::WorkspaceIoLimits::default(),
            &|| false,
        );

        assert_eq!("snapshot_stale", stale.structured.unwrap()["reason_code"]);
        assert_eq!("new version\n", std::fs::read_to_string(path).unwrap());
    }
}
