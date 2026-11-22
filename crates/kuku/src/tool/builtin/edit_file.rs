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

use super::file_change;

pub(crate) fn line_change_counts(before: &str, after: &str) -> Option<(usize, usize)> {
    file_change::summarize(before, after).map(|change| (change.added_lines, change.removed_lines))
}

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
    let change = file_change::facts(&content, &edited);
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
mod tests;
