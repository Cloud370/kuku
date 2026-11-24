use std::borrow::Cow;
use std::fmt::Write;
use std::io;
use std::path::Path;

use serde_json::Value;

use crate::event::StoredEvent;
use crate::execution::{FileCommitState, FileMutationFacts, FileMutationKind};
use crate::tool::ToolResultEnvelope;

use super::common::{
    ExecutorObservation, FileCancellation, IoLimit, ResolvedPath, WriteEvidence,
    WriteSnapshotLookup, append_text, content_hash, ensure_active, expected_version,
    file_read_error, mutation_preview, plural, publish_file, read_file_as_utf8_bounded,
    require_version, resolve_path, snapshot_stale_error, text_matches,
};

use super::file_change;

pub(crate) fn line_change_counts(before: &str, after: &str) -> Option<(usize, usize)> {
    file_change::summarize(before, after).map(|change| (change.added_lines, change.removed_lines))
}

struct EditRequest<'a> {
    path: &'a str,
    old_text: &'a str,
    new_text: &'a str,
    replace_all: bool,
}

struct EditTextCandidate<'a> {
    old_text: Cow<'a, str>,
    new_text: Cow<'a, str>,
}

pub(crate) fn edit_file(
    args: &Value,
    workspace: &Path,
    conversation: &crate::conversation::address::ConversationAddress,
    prior_events: &[StoredEvent],
    text_bytes: usize,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    edit_file_with_evidence(
        args,
        workspace,
        WriteEvidence::Model {
            conversation,
            events: prior_events,
        },
        text_bytes,
        cancelled,
    )
}

pub(crate) fn edit_file_for_executor(
    args: &Value,
    workspace: &Path,
    reads: &[ExecutorObservation],
    text_bytes: usize,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    edit_file_with_evidence(
        args,
        workspace,
        WriteEvidence::Executor(reads),
        text_bytes,
        cancelled,
    )
}

fn edit_file_with_evidence(
    args: &Value,
    workspace: &Path,
    evidence: WriteEvidence<'_>,
    text_bytes: usize,
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
    let resolved = match resolve_path(workspace, request.path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    if request.old_text.len() > text_bytes || request.new_text.len() > text_bytes {
        let error = io::Error::other(IoLimit("edit arguments exceed the text budget"));
        return file_read_error(Path::new(&resolved.relative), error);
    }
    let content = match read_file_as_utf8_bounded(&resolved.path, text_bytes, cancelled) {
        Ok(content) => content,
        Err(err) => return err,
    };
    let current_hash = content_hash(content.as_bytes());
    if let Err(result) = require_version(&resolved.relative, expected, Some(&current_hash)) {
        return result;
    }
    let text_candidates = match edit_text_candidates(&content, &request, text_bytes) {
        Ok(candidates) => candidates,
        Err(error) => return file_read_error(Path::new(&resolved.relative), error),
    };
    let edited = if request.replace_all {
        if let Err(result) = require_full_snapshot(&resolved, &current_hash, &evidence) {
            return result;
        }
        replace_matches(
            &content,
            all_replacements(&content, &text_candidates),
            text_bytes,
            cancelled,
        )
    } else {
        let replacement = match select_unique_replacement(
            &content,
            &text_candidates,
            &resolved,
            &current_hash,
            &evidence,
            cancelled,
        ) {
            Ok(replacement) => replacement,
            Err(result) => return result,
        };
        replace_matches(
            &content,
            std::iter::once(replacement),
            text_bytes,
            cancelled,
        )
    };
    let (edited, replacement_count) = match edited {
        Ok((_, 0)) => return missing_old_text(&resolved),
        Ok(result) => result,
        Err(error) => return file_read_error(Path::new(&resolved.relative), error),
    };
    let change = file_change::facts(&content, &edited);
    let publication = match publish_file(
        &resolved.path,
        Some(&current_hash),
        edited.as_bytes(),
        text_bytes as u64,
        cancelled,
    ) {
        Ok(publication) => publication,
        Err(error) => return error.result(&resolved.relative),
    };
    let content_hash_after = content_hash(edited.as_bytes());
    let content_changed = current_hash != content_hash_after;
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
    let mut result = publication.result(
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
    );
    if result.status == "ok" && content_changed {
        result.model_content.push_str(". File change committed.");
        if let Some(change) = result
            .structured
            .as_ref()
            .and_then(|value| value.get("change"))
        {
            append_change_preview(&mut result.model_content, change);
        }
    } else if result.status == "ok" {
        result.model_content.push_str(". File contents unchanged.");
    }
    result
}

fn append_change_preview(receipt: &mut String, change: &Value) {
    if let Some(line) = change["first_changed_line"].as_u64() {
        let _ = write!(receipt, "\nFirst changed line: {line}.");
    } else if let (Some(preview), Some(line_numbers)) = (
        change["preview"].as_str(),
        change["preview_line_numbers"].as_array(),
    ) {
        for (line, number) in preview.lines().zip(line_numbers) {
            let _ = write!(receipt, "\n{number}: {line}");
        }
        if change["preview_truncated"] == true {
            receipt.push_str("\nChange preview truncated.");
        }
    }
}

type Replacement<'a> = (usize, usize, &'a str);

fn select_unique_replacement<'a>(
    content: &str,
    candidates: &'a [EditTextCandidate<'_>],
    resolved: &ResolvedPath,
    current_hash: &str,
    evidence: &WriteEvidence<'_>,
    cancelled: &FileCancellation<'_>,
) -> Result<Replacement<'a>, ToolResultEnvelope> {
    let mut found = None;
    let mut count = 0;
    for candidate in candidates {
        for offset in text_matches(content, &candidate.old_text, true) {
            ensure_active(cancelled)
                .map_err(|error| file_read_error(Path::new(&resolved.relative), error))?;
            count += 1;
            found = Some(replacement(offset, candidate));
        }
    }
    if count <= 1 {
        return found.ok_or_else(|| missing_old_text(resolved));
    }
    let ambiguous = || {
        ToolResultEnvelope::error(
            format!(
                "failed: old_text matched {count} times in {}",
                resolved.relative
            ),
            "old_text is not unique; provide more context or read the intended occurrence",
        )
    };
    let mut visible = None;
    for candidate in candidates {
        let text = &candidate.old_text;
        let WriteSnapshotLookup::Found(snapshot) =
            evidence.find(&resolved.path, false, Some(text), current_hash)
        else {
            continue;
        };
        if snapshot.content_hash != current_hash {
            continue;
        }
        for offset in snapshot.visible_matches(content, text).take(2) {
            if visible.replace(replacement(offset, candidate)).is_some() {
                return Err(ambiguous());
            }
        }
    }
    visible.ok_or_else(ambiguous)
}

fn missing_old_text(resolved: &ResolvedPath) -> ToolResultEnvelope {
    ToolResultEnvelope::error(
        format!("failed: old_text not found in {}", resolved.relative),
        "old_text was not found",
    )
}

fn require_full_snapshot(
    resolved: &ResolvedPath,
    current_hash: &str,
    evidence: &WriteEvidence<'_>,
) -> Result<(), ToolResultEnvelope> {
    let relative = &resolved.relative;
    match evidence.find(&resolved.path, true, None, current_hash) {
        WriteSnapshotLookup::Found(snapshot) if snapshot.content_hash == current_hash => Ok(()),
        WriteSnapshotLookup::Found(snapshot) => {
            Err(snapshot_stale_error(relative, snapshot.event_id))
        }
        WriteSnapshotLookup::Rejected(reason) => Err(ToolResultEnvelope::error_with_reason(
            format!("failed: fully read {relative} before editing"),
            format!(
                "replace_all requires a current full read_file snapshot or your own confirmed full write of {relative}"
            ),
            reason,
        )),
    }
}

fn replacement<'a>(offset: usize, candidate: &'a EditTextCandidate<'_>) -> Replacement<'a> {
    (
        offset,
        offset + candidate.old_text.len(),
        &candidate.new_text,
    )
}

fn all_replacements<'a>(
    content: &'a str,
    candidates: &'a [EditTextCandidate<'_>],
) -> impl Iterator<Item = Replacement<'a>> {
    let stream = |candidate: &'a EditTextCandidate<'_>| {
        text_matches(content, &candidate.old_text, false)
            .map(move |offset| replacement(offset, candidate))
    };
    let mut left = stream(&candidates[0]).peekable();
    let mut right = candidates.get(1).into_iter().flat_map(stream).peekable();
    std::iter::from_fn(move || match (left.peek(), right.peek()) {
        (Some(a), Some(b)) if a.0 > b.0 => right.next(),
        (Some(_), _) => left.next(),
        (None, _) => right.next(),
    })
}

fn replace_matches<'a>(
    content: &str,
    replacements: impl Iterator<Item = Replacement<'a>>,
    text_bytes: usize,
    cancelled: &FileCancellation<'_>,
) -> io::Result<(String, usize)> {
    let mut edited = String::new();
    let mut cursor = 0;
    let mut replacement_count = 0;
    for (start, end, replacement) in replacements {
        ensure_active(cancelled)?;
        if start < cursor {
            continue;
        }
        append_text(&mut edited, &content[cursor..start], text_bytes)?;
        append_text(&mut edited, replacement, text_bytes)?;
        cursor = end;
        replacement_count += 1;
    }
    if replacement_count != 0 {
        append_text(&mut edited, &content[cursor..], text_bytes)?;
    }
    Ok((edited, replacement_count))
}

fn edit_text_candidates<'a>(
    content: &str,
    request: &EditRequest<'a>,
    text_bytes: usize,
) -> io::Result<Vec<EditTextCandidate<'a>>> {
    let EditRequest {
        old_text, new_text, ..
    } = *request;
    if old_text.contains('\n') && !old_text.contains("\r\n") {
        return Ok(vec![
            EditTextCandidate {
                old_text: crlf(old_text, text_bytes)?,
                new_text: crlf(new_text, text_bytes)?,
            },
            EditTextCandidate {
                old_text: Cow::Borrowed(old_text),
                new_text: Cow::Borrowed(new_text),
            },
        ]);
    }

    let file_uses_crlf =
        content.contains("\r\n") && content.split("\r\n").all(|segment| !segment.contains('\n'));
    Ok(vec![EditTextCandidate {
        old_text: Cow::Borrowed(old_text),
        new_text: if old_text.contains("\r\n") || file_uses_crlf {
            crlf(new_text, text_bytes)?
        } else {
            Cow::Borrowed(new_text)
        },
    }])
}

fn crlf(text: &str, text_bytes: usize) -> io::Result<Cow<'_, str>> {
    let needs_crlf = |line: &str| line.ends_with('\n') && !line.ends_with("\r\n");
    if !text.split_inclusive('\n').any(needs_crlf) {
        return Ok(Cow::Borrowed(text));
    }
    let mut result = String::new();
    for line in text.split_inclusive('\n') {
        if needs_crlf(line) {
            append_text(&mut result, &line[..line.len() - 1], text_bytes)?;
            append_text(&mut result, "\r\n", text_bytes)?;
        } else {
            append_text(&mut result, line, text_bytes)?;
        }
    }
    Ok(Cow::Owned(result))
}

fn edit_file_request(args: &Value) -> Result<EditRequest<'_>, ToolResultEnvelope> {
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
        path,
        old_text,
        new_text,
        replace_all: args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests;
