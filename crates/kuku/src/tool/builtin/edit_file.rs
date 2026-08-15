use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::event::StoredEvent;
use crate::tool::{ToolErrorReason, ToolResultEnvelope};

use super::common::{
    capability_relative_path, content_hash, find_write_snapshot, plural, read_file_as_utf8,
    require_brief, resolve_path, text_match_offsets, write_atomically, WriteSnapshot,
};

struct EditRequest {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: bool,
    _brief: String,
}

const EDIT_FILE_MAX_BYTES: usize = 16 * 1024 * 1024;

struct EditTextCandidate {
    old_text: String,
    new_text: String,
}

struct AuthorizedEditCandidate {
    candidate_index: usize,
    offsets: Vec<usize>,
}

pub(crate) fn edit_file(
    args: &Value,
    workspace: &Path,
    conversation: &crate::conversation::address::ConversationAddress,
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

    let (content, bytes) = match read_file_as_utf8(&resolved.path) {
        Ok(result) => result,
        Err(err) => return err,
    };
    let text_candidates = edit_text_candidates(
        &content,
        request.old_text.as_str(),
        request.new_text.as_str(),
    );
    let current_hash = content_hash(&bytes);
    let mut authorized_candidates = Vec::new();
    let mut stale_snapshot = None;
    let mut saw_fresh_authorization = false;
    let mut rejection = ToolErrorReason::SnapshotRequired;
    for (index, candidate) in text_candidates.iter().enumerate() {
        match find_write_snapshot(
            prior_events,
            conversation,
            &resolved.path,
            request.replace_all,
            Some(&candidate.old_text),
            Some(&current_hash),
        ) {
            super::common::WriteSnapshotLookup::Found(snapshot) => {
                if snapshot.content_hash == current_hash {
                    saw_fresh_authorization = true;
                    let offsets = text_match_offsets(&content, &candidate.old_text);
                    if !offsets.is_empty() {
                        authorized_candidates.push(AuthorizedEditCandidate {
                            candidate_index: index,
                            offsets,
                        });
                    }
                } else if stale_snapshot.is_none() {
                    stale_snapshot = Some(snapshot);
                }
            }
            super::common::WriteSnapshotLookup::Rejected(reason) => rejection = reason,
        }
    }
    if authorized_candidates.is_empty() {
        if saw_fresh_authorization {
            return ToolResultEnvelope::error(
                format!("failed: old_text not found in {}", resolved.relative),
                "old_text was not found".to_string(),
            );
        }
        if let Some(snapshot) = stale_snapshot {
            return snapshot_stale_error(&resolved.relative, &snapshot);
        }
        return snapshot_lookup_error(&resolved.relative, rejection);
    }

    let replacement_count = authorized_candidates
        .iter()
        .map(|candidate| candidate.offsets.len())
        .sum::<usize>();
    if replacement_count > 1 && !request.replace_all {
        return ToolResultEnvelope::error(
            format!(
                "failed: old_text matched {replacement_count} times in {}",
                resolved.relative
            ),
            "old_text is not unique; provide more context or set replace_all=true".to_string(),
        );
    }

    let edited = replace_authorized_candidates(&content, &text_candidates, &authorized_candidates);
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

pub(crate) fn edit_file_with_capability(
    args: &Value,
    capability: &dyn crate::query::WorkspaceQueryCapability,
    prior_events: &[StoredEvent],
) -> ToolResultEnvelope {
    let request = match edit_file_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let path = match capability_relative_path(&request.path, false) {
        Ok(path) => path,
        Err(result) => return result,
    };
    if crate::util::path::is_blocked_relative_path(&path) {
        return ToolResultEnvelope::blocked(
            format!("blocked: path is not writable: {path}"),
            format!("path is blocked by write guard: {path}"),
        );
    }
    let bytes = match capability.read_file(&path, EDIT_FILE_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: {error}"),
                format!("error reading file: {path}"),
            )
        }
    };
    let content = match String::from_utf8(bytes.clone()) {
        Ok(content) => content,
        Err(_) => {
            return ToolResultEnvelope::error(
                format!("failed: file is not valid UTF-8: {path}"),
                format!("file is not valid UTF-8: {path}"),
            )
        }
    };
    let text_candidates = edit_text_candidates(
        &content,
        request.old_text.as_str(),
        request.new_text.as_str(),
    );
    let current_hash = content_hash(&bytes);
    let identity_path = std::path::PathBuf::from("workspace").join(&path);
    let mut authorized_candidates = Vec::new();
    let mut stale_snapshot = None;
    let mut saw_fresh_authorization = false;
    let mut rejection = ToolErrorReason::SnapshotRequired;
    for (index, candidate) in text_candidates.iter().enumerate() {
        match find_write_snapshot(
            prior_events,
            &crate::conversation::address::ConversationAddress::MAIN,
            &identity_path,
            request.replace_all,
            Some(&candidate.old_text),
            Some(&current_hash),
        ) {
            super::common::WriteSnapshotLookup::Found(snapshot) => {
                if snapshot.content_hash == current_hash {
                    saw_fresh_authorization = true;
                    let offsets = text_match_offsets(&content, &candidate.old_text);
                    if !offsets.is_empty() {
                        authorized_candidates.push(AuthorizedEditCandidate {
                            candidate_index: index,
                            offsets,
                        });
                    }
                } else if stale_snapshot.is_none() {
                    stale_snapshot = Some(snapshot);
                }
            }
            super::common::WriteSnapshotLookup::Rejected(reason) => rejection = reason,
        }
    }
    if authorized_candidates.is_empty() {
        if saw_fresh_authorization {
            return ToolResultEnvelope::error(
                format!("failed: old_text not found in {path}"),
                "old_text was not found".to_string(),
            );
        }
        if let Some(snapshot) = stale_snapshot {
            return snapshot_stale_error(&path, &snapshot);
        }
        return snapshot_lookup_error(&path, rejection);
    }

    let replacement_count = authorized_candidates
        .iter()
        .map(|candidate| candidate.offsets.len())
        .sum::<usize>();
    if replacement_count > 1 && !request.replace_all {
        return ToolResultEnvelope::error(
            format!("failed: old_text matched {replacement_count} times in {path}"),
            "old_text is not unique; provide more context or set replace_all=true".to_string(),
        );
    }

    let edited = replace_authorized_candidates(&content, &text_candidates, &authorized_candidates);
    if let Err(error) = capability.write_file(&path, edited.as_bytes(), EDIT_FILE_MAX_BYTES) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error writing file: {path}"),
        );
    }

    let raw_text_after = edited;
    let bytes_written = raw_text_after.len();
    let content_hash_after = content_hash(raw_text_after.as_bytes());
    let summary = format!(
        "edited {path}, {replacement_count} replacement{}",
        plural(replacement_count)
    );
    ToolResultEnvelope::ok(
        summary.clone(),
        summary,
        serde_json::json!({
            "kind": "file_edit",
            "path": path,
            "canonical_path": identity_path.to_string_lossy(),
            "replacement_count": replacement_count,
            "bytes_written": bytes_written,
            "content_hash": content_hash_after,
            "content_hash_after": content_hash_after,
            "raw_text_after": raw_text_after,
        }),
    )
}

fn snapshot_lookup_error(relative: &str, reason: ToolErrorReason) -> ToolResultEnvelope {
    let model_content = match reason {
        ToolErrorReason::OldTextNotVisible => format!(
            "old_text is outside the visible read_file snapshot; read the matching lines from {relative} before editing"
        ),
        ToolErrorReason::FullSnapshotRequired => format!(
            "replace_all requires a full read_file snapshot; read all of {relative} before editing"
        ),
        ToolErrorReason::SnapshotRequired | ToolErrorReason::SnapshotStale => format!(
            "edit_file requires a prior successful read_file snapshot for {relative}"
        ),
    };
    ToolResultEnvelope::error_with_reason(
        format!("failed: read {relative} before editing"),
        model_content,
        reason,
    )
}

fn snapshot_stale_error(relative: &str, snapshot: &WriteSnapshot) -> ToolResultEnvelope {
    ToolResultEnvelope::error_with_reason(
        format!(
            "failed: {relative} changed since event {}",
            snapshot.event_id
        ),
        format!("file changed since it was read; read {relative} again before editing"),
        ToolErrorReason::SnapshotStale,
    )
}

fn replace_authorized_candidates(
    content: &str,
    candidates: &[EditTextCandidate],
    authorized: &[AuthorizedEditCandidate],
) -> String {
    let mut replacements = authorized
        .iter()
        .flat_map(|authorized_candidate| {
            let candidate = &candidates[authorized_candidate.candidate_index];
            authorized_candidate.offsets.iter().map(move |offset| {
                (
                    *offset,
                    *offset + candidate.old_text.len(),
                    candidate.new_text.as_str(),
                )
            })
        })
        .collect::<Vec<_>>();
    replacements.sort_by_key(|(start, _, _)| *start);

    let mut edited = String::with_capacity(content.len());
    let mut cursor = 0;
    for (start, end, replacement) in replacements {
        edited.push_str(&content[cursor..start]);
        edited.push_str(replacement);
        cursor = end;
    }
    edited.push_str(&content[cursor..]);
    edited
}

fn edit_text_candidates(content: &str, old_text: &str, new_text: &str) -> Vec<EditTextCandidate> {
    let file_uses_crlf =
        content.contains("\r\n") && content.split("\r\n").all(|segment| !segment.contains('\n'));
    let crlf_new_text = new_text.replace("\r\n", "\n").replace('\n', "\r\n");
    if old_text.contains('\n') && !old_text.contains("\r\n") {
        let crlf_old_text = old_text.replace('\n', "\r\n");
        return vec![
            EditTextCandidate {
                old_text: crlf_old_text,
                new_text: crlf_new_text.clone(),
            },
            EditTextCandidate {
                old_text: old_text.to_string(),
                new_text: new_text.to_string(),
            },
        ];
    }

    vec![EditTextCandidate {
        old_text: old_text.to_string(),
        new_text: if old_text.contains("\r\n") || file_uses_crlf {
            crlf_new_text
        } else {
            new_text.to_string()
        },
    }]
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
    let brief = require_brief("edit_file", args)?;
    Ok(EditRequest {
        path: path.to_string(),
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
        replace_all: args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _brief: brief,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{read_snapshot_event, workspace};
    use super::*;

    fn edit_file(
        args: &Value,
        workspace: &Path,
        prior_events: &[StoredEvent],
    ) -> ToolResultEnvelope {
        super::edit_file(
            args,
            workspace,
            &crate::conversation::address::ConversationAddress::MAIN,
            prior_events,
        )
    }

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
        assert_eq!(missing.structured.as_ref().unwrap()["kind"], "error");
        assert_eq!(
            missing.structured.as_ref().unwrap()["reason_code"],
            "snapshot_required"
        );
        assert!(missing
            .model_content
            .contains("prior successful read_file snapshot"));

        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            b"alpha\nbeta\n",
            true,
            "alpha\nbeta\n",
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), "changed\nbeta\n").unwrap();
        let stale = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "beta", "new_text": "gamma", "brief": "change beta"}),
            dir.path(),
            &[snapshot],
        );
        assert_eq!(stale.status, "error");
        assert_eq!(
            stale.structured.as_ref().unwrap()["reason_code"],
            "snapshot_stale"
        );
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
            "alpha\nbeta\nalpha\n",
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
            "alpha\ngamma\nalpha\n",
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

    #[test]
    fn partial_snapshot_authorizes_multiline_visible_raw_text() {
        let dir = workspace();
        let content = b"alpha\nbeta\ngamma\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            false,
            "alpha\nbeta\n",
            "1\talpha\n2\tbeta",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega",
                "brief": "replace two visible lines"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "omega\ngamma\n"
        );
    }

    #[test]
    fn partial_snapshot_maps_model_lf_multiline_edit_to_visible_crlf_bytes() {
        let dir = workspace();
        let content = b"alpha\r\nbeta\r\ngamma\r\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            false,
            "alpha\r\nbeta\r\n",
            "1\talpha\n2\tbeta",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega\ndelta",
                "brief": "replace visible CRLF lines"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            b"omega\r\ndelta\r\ngamma\r\n"
        );
    }

    #[test]
    fn crlf_file_normalizes_multiline_replacement_for_single_line_match() {
        let dir = workspace();
        let content = b"alpha\r\nbeta\r\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            false,
            "alpha\r\n",
            "1\talpha",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha",
                "new_text": "omega\ndelta",
                "brief": "expand one CRLF line"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            b"omega\r\ndelta\r\nbeta\r\n"
        );
    }

    #[test]
    fn leading_model_lf_does_not_match_inside_crlf() {
        let dir = workspace();
        let content = b"alpha\r\nbeta\r\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            false,
            "alpha\r\nbeta\r\n",
            "1\talpha\n2\tbeta",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "\nbeta",
                "new_text": "\nomega",
                "brief": "rename the second CRLF line"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            b"alpha\r\nomega\r\n"
        );
    }

    #[test]
    fn visible_crlf_candidate_wins_over_hidden_lf_candidate() {
        let dir = workspace();
        let content = b"alpha\r\nbeta\r\nalpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            false,
            "alpha\r\nbeta\r\n",
            "1\talpha\n2\tbeta",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega\ndelta",
                "brief": "replace the visible CRLF lines"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            b"omega\r\ndelta\r\nalpha\nbeta\n"
        );
    }

    #[test]
    fn full_snapshot_replace_all_replaces_lf_and_crlf_candidates() {
        let dir = workspace();
        let content = b"alpha\r\nbeta\r\nalpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            true,
            "alpha\r\nbeta\r\nalpha\nbeta\n",
            "1\talpha\n2\tbeta\n3\talpha\n4\tbeta",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega\ndelta",
                "replace_all": true,
                "brief": "replace both line-ending forms"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.structured.as_ref().unwrap()["replacement_count"], 2);
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            b"omega\r\ndelta\r\nomega\ndelta\n"
        );
    }

    #[test]
    fn full_snapshot_single_edit_rejects_lf_and_crlf_ambiguity() {
        let dir = workspace();
        let content = b"alpha\r\nbeta\r\nalpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            true,
            "alpha\r\nbeta\r\nalpha\nbeta\n",
            "1\talpha\n2\tbeta\n3\talpha\n4\tbeta",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega\ndelta",
                "brief": "replace one line-ending form"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "error");
        assert!(result.model_content.contains("not unique"));
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            content
        );
    }

    #[test]
    fn line_ending_change_after_partial_read_is_stale() {
        let dir = workspace();
        let original = b"alpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            original,
            false,
            "alpha\nbeta\n",
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), b"alpha\r\nbeta\r\n").unwrap();

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega\ndelta",
                "brief": "replace stale lines"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "error");
        assert_eq!(
            result.structured.as_ref().unwrap()["reason_code"],
            "snapshot_stale"
        );
    }

    #[test]
    fn fresh_lf_snapshot_wins_over_stale_crlf_candidate() {
        let dir = workspace();
        let stale_content = b"alpha\r\nbeta\r\nold\n";
        std::fs::write(dir.path().join("README.md"), stale_content).unwrap();
        let stale_snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            stale_content,
            false,
            "alpha\r\nbeta\r\n",
            "1\talpha\n2\tbeta",
        );

        let current = b"alpha\r\nbeta\r\nalpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), current).unwrap();
        let fresh_snapshot = read_snapshot_event(
            18,
            dir.path(),
            "README.md",
            current,
            false,
            "alpha\nbeta\n",
            "3\talpha\n4\tbeta",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega\ndelta",
                "brief": "replace fresh LF lines"
            }),
            dir.path(),
            &[stale_snapshot, fresh_snapshot],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            b"alpha\r\nbeta\r\nomega\ndelta\n"
        );
    }

    #[test]
    fn stale_partial_snapshot_is_rejected_even_when_visible_text_still_matches() {
        let dir = workspace();
        let original = b"alpha\nbeta\ngamma\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            original,
            false,
            "alpha\nbeta\n",
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), "alpha\nbeta\nchanged\n").unwrap();

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha\nbeta",
                "new_text": "omega",
                "brief": "replace two visible lines"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "error");
        assert_eq!(
            result.structured.as_ref().unwrap()["reason_code"],
            "snapshot_stale"
        );
        assert!(result.model_content.contains("read README.md again"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "alpha\nbeta\nchanged\n"
        );
    }

    #[test]
    fn partial_snapshot_rejects_old_text_outside_visible_raw_text() {
        let dir = workspace();
        let content = b"alpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            false,
            "alpha\n",
            "1\talpha",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "beta",
                "new_text": "gamma",
                "brief": "change hidden line"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.structured.as_ref().unwrap()["kind"], "error");
        assert_eq!(
            result.structured.as_ref().unwrap()["reason_code"],
            "old_text_not_visible"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "alpha\nbeta\n"
        );
    }

    #[test]
    fn partial_snapshot_cannot_authorize_replace_all_beyond_visible_range() {
        let dir = workspace();
        let content = b"alpha\nhidden\nalpha\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            false,
            "alpha\n",
            "1\talpha",
        );

        let result = edit_file(
            &serde_json::json!({
                "path": "README.md",
                "old_text": "alpha",
                "new_text": "omega",
                "replace_all": true,
                "brief": "replace every alpha"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!(result.status, "error");
        assert_eq!(
            result.structured.as_ref().unwrap()["reason_code"],
            "full_snapshot_required"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "alpha\nhidden\nalpha\n"
        );
    }
}
