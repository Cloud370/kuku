use std::path::Path;

use serde_json::Value;

use crate::event::StoredEvent;
use crate::execution::{FileCommitState, FileMutationFacts, FileMutationKind};
use crate::tool::ToolResultEnvelope;

use super::common::{
    content_hash, expected_version, mutation_preview, plural, publish_file, read_file_as_utf8,
    require_version, resolve_path, snapshot_stale_error, text_match_offsets, ExecutorObservation,
    FileCancellation, ResolvedPath, WriteEvidence, WriteSnapshotLookup,
};

mod change;

struct EditRequest {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: bool,
}

struct EditTextCandidate {
    old_text: String,
    new_text: String,
}

struct EditMatches<'a> {
    candidate: &'a EditTextCandidate,
    offsets: Vec<usize>,
}

pub(crate) fn edit_file(
    args: &Value,
    workspace: &Path,
    conversation: &crate::conversation::address::ConversationAddress,
    prior_events: &[StoredEvent],
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    edit_file_with_evidence(
        args,
        workspace,
        WriteEvidence::Model {
            conversation,
            events: prior_events,
        },
        cancelled,
    )
}

pub(crate) fn edit_file_for_executor(
    args: &Value,
    workspace: &Path,
    reads: &[ExecutorObservation],
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    edit_file_with_evidence(args, workspace, WriteEvidence::Executor(reads), cancelled)
}

fn edit_file_with_evidence(
    args: &Value,
    workspace: &Path,
    evidence: WriteEvidence<'_>,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    let request = match edit_file_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let expected = match expected_version(args) {
        Ok(expected) => expected,
        Err(result) => return result,
    };
    let resolved = match resolve_path(workspace, &request.path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    let content = match read_file_as_utf8(&resolved.path, cancelled) {
        Ok(content) => content,
        Err(err) => return err,
    };
    let current_hash = content_hash(content.as_bytes());
    if let Err(result) = require_version(&resolved.relative, expected, Some(&current_hash)) {
        return result;
    }
    let text_candidates = edit_text_candidates(
        &content,
        request.old_text.as_str(),
        request.new_text.as_str(),
    );
    let matches = match select_edit_matches(
        &content,
        &text_candidates,
        &resolved,
        &current_hash,
        &evidence,
        request.replace_all,
    ) {
        Ok(matches) => matches,
        Err(result) => return result,
    };

    let (edited, replacement_count) = replace_matches(&content, &matches);
    let change = change::summarize(&content, &edited);
    let publication = match publish_file(
        &resolved.path,
        Some(&current_hash),
        edited.as_bytes(),
        cancelled,
    ) {
        Ok(publication) => publication,
        Err(error) => return error.result(&resolved.relative),
    };
    let content_hash_after = content_hash(edited.as_bytes());
    let mut fields = serde_json::json!({
        "replacement_count": replacement_count,
        "change": change,
        "content_hash": content_hash_after,
    });
    if evidence.is_model() {
        mutation_preview(&edited, &mut fields);
    }
    let summary = format!(
        "edited {}, {replacement_count} replacement{}",
        resolved.relative,
        plural(replacement_count)
    );
    publication.result(
        FileMutationFacts {
            kind: FileMutationKind::FileEdit,
            path: resolved.relative,
            canonical_path: resolved.path.to_string_lossy().into_owned(),
            commit_state: FileCommitState::Committed,
            content_hash_before: Some(current_hash),
            content_hash_after: Some(content_hash_after),
            bytes_written: Some(edited.len()),
            bytes_removed: None,
        },
        summary,
        fields,
    )
}

fn select_edit_matches<'a>(
    content: &str,
    candidates: &'a [EditTextCandidate],
    resolved: &ResolvedPath,
    current_hash: &str,
    evidence: &WriteEvidence<'_>,
    replace_all: bool,
) -> Result<Vec<EditMatches<'a>>, ToolResultEnvelope> {
    if replace_all {
        require_full_snapshot(resolved, current_hash, evidence)?;
    }
    let mut matches = candidates
        .iter()
        .filter_map(|candidate| {
            let offsets = text_match_offsets(content, &candidate.old_text, !replace_all);
            (!offsets.is_empty()).then_some(EditMatches { candidate, offsets })
        })
        .collect::<Vec<_>>();
    let match_count = matches
        .iter()
        .map(|entry| entry.offsets.len())
        .sum::<usize>();
    if match_count == 0 {
        return Err(ToolResultEnvelope::error(
            format!("failed: old_text not found in {}", resolved.relative),
            "old_text was not found",
        ));
    }
    if !replace_all && match_count > 1 {
        for entry in &mut matches {
            let old_text = &entry.candidate.old_text;
            entry.offsets = match evidence.find(&resolved.path, false, Some(old_text), current_hash)
            {
                WriteSnapshotLookup::Found(snapshot) if snapshot.content_hash == current_hash => {
                    snapshot.restrict_matches(content, old_text, std::mem::take(&mut entry.offsets))
                }
                _ => Vec::new(),
            };
        }
        if matches
            .iter()
            .map(|entry| entry.offsets.len())
            .sum::<usize>()
            != 1
        {
            return Err(ToolResultEnvelope::error(
                format!(
                    "failed: old_text matched {match_count} times in {}",
                    resolved.relative
                ),
                "old_text is not unique; provide more context or read the intended occurrence",
            ));
        }
    }
    Ok(matches)
}

fn require_full_snapshot(
    resolved: &ResolvedPath,
    current_hash: &str,
    evidence: &WriteEvidence<'_>,
) -> Result<(), ToolResultEnvelope> {
    let relative = &resolved.relative;
    match evidence.find(&resolved.path, true, None, current_hash) {
        WriteSnapshotLookup::Found(snapshot) if snapshot.content_hash == current_hash => Ok(()),
        WriteSnapshotLookup::Found(snapshot) => Err(snapshot_stale_error(relative, snapshot.event_id)),
        WriteSnapshotLookup::Rejected(reason) => Err(ToolResultEnvelope::error_with_reason(
            format!("failed: fully read {relative} before editing"),
            format!("replace_all requires a current full read_file snapshot or your own confirmed full write of {relative}"),
            reason,
        )),
    }
}

fn replace_matches(content: &str, matches: &[EditMatches<'_>]) -> (String, usize) {
    let mut replacements = matches
        .iter()
        .flat_map(|entry| {
            let candidate = entry.candidate;
            entry.offsets.iter().map(move |offset| {
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
    let mut replacement_count = 0;
    for (start, end, replacement) in replacements {
        if start < cursor {
            continue;
        }
        edited.push_str(&content[cursor..start]);
        edited.push_str(replacement);
        cursor = end;
        replacement_count += 1;
    }
    edited.push_str(&content[cursor..]);
    (edited, replacement_count)
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
    Ok(EditRequest {
        path: path.to_string(),
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
        replace_all: args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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
            &|| false,
        )
    }

    #[test]
    fn edit_file_replaces_unique_text_without_a_read() {
        let dir = workspace();
        let path = dir.path().join("README.md");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();

        let result = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega"}),
            dir.path(),
            &[],
        );

        assert_eq!("ok", result.status);
        assert_eq!("omega\nbeta\n", std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn consecutive_exact_edits_preserve_other_changes_and_reject_missing_text() {
        let dir = workspace();
        let path = dir.path().join("README.md");
        let original = b"alpha\nbeta\n";
        std::fs::write(&path, original).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            original,
            true,
            "alpha\nbeta\n",
            "1\talpha\n2\tbeta",
        );
        let events = std::slice::from_ref(&snapshot);
        let first = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega"}),
            dir.path(),
            events,
        );
        assert_eq!("ok", first.status);

        let second = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "beta", "new_text": "gamma"}),
            dir.path(),
            events,
        );
        assert_eq!("ok", second.status);
        assert_eq!("omega\ngamma\n", std::fs::read_to_string(&path).unwrap());

        let missing = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "other"}),
            dir.path(),
            events,
        );
        assert_eq!("error", missing.status);
        assert_eq!("old_text was not found", missing.model_content);
        assert_eq!("omega\ngamma\n", std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn single_edit_rejects_overlapping_utf8_matches() {
        let dir = workspace();
        let path = dir.path().join("README.md");
        std::fs::write(&path, "ébébé").unwrap();
        let result = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "ébé", "new_text": "changed"}),
            dir.path(),
            &[],
        );

        assert_eq!("error", result.status);
        assert!(result.model_content.contains("not unique"));
        assert_eq!("ébébé", std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn overlapping_search_skips_lf_inside_crlf_without_losing_the_next_match() {
        assert_eq!(vec![2], text_match_offsets("\r\n\n\n", "\n\n", true));
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
        assert_eq!(structured["change"]["added_lines"], 1);
        assert_eq!(structured["change"]["removed_lines"], 1);
        assert_eq!(structured["change"]["preview"], "-beta\n+gamma");
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
        assert_eq!(all.structured.as_ref().unwrap()["change"]["added_lines"], 2);
        assert_eq!(
            all.structured.as_ref().unwrap()["change"]["removed_lines"],
            2
        );
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
    fn unique_match_after_line_ending_change_preserves_current_crlf() {
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
                "brief": "replace the matching lines"
            }),
            dir.path(),
            &[snapshot],
        );

        assert_eq!("ok", result.status);
        assert_eq!(
            b"omega\r\ndelta\r\n",
            std::fs::read(dir.path().join("README.md"))
                .unwrap()
                .as_slice()
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
        let mut fresh_snapshot = read_snapshot_event(
            18,
            dir.path(),
            "README.md",
            current,
            false,
            "alpha\nbeta\n",
            "3\talpha\n4\tbeta",
        );
        if let crate::event::EventPayload::ToolResult {
            structured: Some(data),
            ..
        } = &mut fresh_snapshot.payload
        {
            data["start_line"] = 3.into();
        }

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
    fn explicit_version_rejects_a_unique_edit_after_an_unrelated_change() {
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
                "expected_version": content_hash(original),
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
    fn stale_partial_snapshot_cannot_disambiguate_repeated_text() {
        let dir = workspace();
        let path = dir.path().join("README.md");
        let original = b"alpha\nbeta\nalpha\n";
        std::fs::write(&path, original).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            original,
            false,
            "alpha\n",
            "1\talpha",
        );
        std::fs::write(&path, "alpha\nchanged\nalpha\n").unwrap();

        let result = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "other"}),
            dir.path(),
            &[snapshot],
        );

        assert_eq!("error", result.status);
        assert!(result.model_content.contains("not unique"));
        assert_eq!(
            "alpha\nchanged\nalpha\n",
            std::fs::read_to_string(path).unwrap()
        );
    }

    #[test]
    fn partial_snapshot_does_not_limit_a_unique_match() {
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

        assert_eq!("ok", result.status);
        assert_eq!(
            "alpha\ngamma\n",
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
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

    #[test]
    fn stale_full_snapshot_cannot_authorize_replace_all_even_for_a_unique_match() {
        let dir = workspace();
        let path = dir.path().join("README.md");
        let original = b"alpha\nbeta\n";
        std::fs::write(&path, original).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            original,
            true,
            "alpha\nbeta\n",
            "1\talpha\n2\tbeta",
        );
        std::fs::write(&path, "alpha\nchanged\n").unwrap();

        let result = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "other", "replace_all": true}),
            dir.path(),
            &[snapshot],
        );

        assert_eq!("error", result.status);
        assert_eq!("snapshot_stale", result.structured.unwrap()["reason_code"]);
        assert_eq!("alpha\nchanged\n", std::fs::read_to_string(path).unwrap());
    }

    #[test]
    fn executor_edit_targets_observed_occurrence_and_reports_committed_versions() {
        let dir = workspace();
        let original = "same\nhidden\nsame\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let (_, read) = crate::tool::builtin::read_file_for_executor(
            &serde_json::json!({"path":"README.md","offset":3,"limit":1}),
            dir.path(),
            80,
            &|| false,
        );
        let read = read.unwrap();

        let result = edit_file_for_executor(
            &serde_json::json!({
                "path":"README.md", "old_text":"same", "new_text":"visible",
                "expected_version": read.content_hash
            }),
            dir.path(),
            std::slice::from_ref(&read),
            &|| false,
        );

        assert_eq!("ok", result.status);
        let expected = "same\nhidden\nvisible\n";
        assert_eq!(
            expected,
            std::fs::read_to_string(dir.path().join("README.md")).unwrap()
        );
        let facts =
            FileMutationFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
        assert_eq!(FileCommitState::Committed, facts.commit_state);
        assert_eq!(
            Some(content_hash(original.as_bytes())),
            facts.content_hash_before
        );
        assert_eq!(
            Some(content_hash(expected.as_bytes())),
            facts.content_hash_after
        );
        assert!(result.structured.unwrap().get("raw_text_after").is_none());
    }

    #[test]
    fn executor_scope_cannot_disambiguate_unobserved_repeated_text() {
        let dir = workspace();
        let original = "observe\nprivate\nprivate\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let (_, read) = crate::tool::builtin::read_file_for_executor(
            &serde_json::json!({"path":"README.md","limit":1}),
            dir.path(),
            80,
            &|| false,
        );
        let result = edit_file_for_executor(
            &serde_json::json!({"path":"README.md","old_text":"private","new_text":"changed"}),
            dir.path(),
            &[read.unwrap()],
            &|| false,
        );
        assert_eq!("error", result.status);
        assert!(result.model_content.contains("not unique"));
        assert_eq!(
            original,
            std::fs::read_to_string(dir.path().join("README.md")).unwrap()
        );
    }

    #[test]
    fn mixed_line_ending_replacements_choose_leftmost_nonoverlapping_match() {
        let dir = workspace();
        let original = "x\r\nx\nx\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let (_, read) = crate::tool::builtin::read_file_for_executor(
            &serde_json::json!({"path":"README.md"}),
            dir.path(),
            80,
            &|| false,
        );
        let result = edit_file_for_executor(
            &serde_json::json!({"path":"README.md","old_text":"x\nx","new_text":"changed","replace_all":true}),
            dir.path(),
            &[read.unwrap()],
            &|| false,
        );
        assert_eq!("ok", result.status);
        assert_eq!(1, result.structured.unwrap()["replacement_count"]);
        assert_eq!(
            "changed\nx\n",
            std::fs::read_to_string(dir.path().join("README.md")).unwrap()
        );
    }
}
