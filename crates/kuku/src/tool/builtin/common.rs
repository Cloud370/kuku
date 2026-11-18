use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::tool::ToolResultEnvelope;
pub(super) use crate::util::path::is_default_excluded_dir;
use crate::util::path::{is_blocked_relative_path, normalize_path_sep};

mod file_io;
mod snapshot;

pub(crate) use file_io::{
    content_hash, file_read_error, file_version, format_content_hash, mutation_preview,
    publish_file, read_file_as_utf8, remove_observed_file, write_atomically, StableFile,
};
#[cfg(test)]
use snapshot::find_write_snapshot;
pub(crate) use snapshot::ExecutorRead;
pub(super) use snapshot::{
    expected_version, find_covering_read, require_version, snapshot_stale_error,
    text_match_offsets, WriteEvidence, WriteSnapshotLookup,
};

pub(super) struct ResolvedPath {
    pub(super) workspace: PathBuf,
    pub(super) path: PathBuf,
    pub(super) relative: String,
}

pub(crate) fn prepare_workspace_path(
    tool: &str,
    args: &Value,
    workspace: &Path,
) -> Result<Option<PathBuf>, ToolResultEnvelope> {
    let optional_path = args.get("path").and_then(Value::as_str);
    let resolved = match tool {
        "find_files" | "search_text" => resolve_path(workspace, optional_path.unwrap_or(".")),
        "read_file" | "edit_file" => resolve_path(workspace, required_path(tool, args)?),
        "write_file" => resolve_write_path(workspace, required_path(tool, args)?),
        "stat_file" => resolve_stat_path(workspace, required_path(tool, args)?),
        "remove_file" => resolve_remove_path(workspace, required_path(tool, args)?),
        _ => return Ok(None),
    }?;
    Ok(Some(resolved.path))
}

pub(super) fn required_path<'a>(
    tool: &str,
    args: &'a Value,
) -> Result<&'a str, ToolResultEnvelope> {
    args.get("path").and_then(Value::as_str).ok_or_else(|| {
        ToolResultEnvelope::error("failed: missing path", format!("{tool} requires path"))
    })
}

pub(super) fn resolve_path(
    workspace: &Path,
    path: &str,
) -> Result<ResolvedPath, ToolResultEnvelope> {
    let (workspace, joined) = workspace_path(workspace, path)?;
    let resolved = joined
        .canonicalize()
        .map_err(|error| path_error(path, error))?;
    checked_resolved_path(workspace, resolved, path)
}

pub(super) fn resolve_write_path(
    workspace: &Path,
    path: &str,
) -> Result<ResolvedPath, ToolResultEnvelope> {
    let resolved = resolve_stat_path(workspace, path)?;
    if resolved.path != resolved.workspace && !resolved.path.parent().is_some_and(Path::is_dir) {
        return Err(ToolResultEnvelope::error(
            format!("failed: parent path not found: {path}"),
            format!("parent directory does not exist: {path}"),
        ));
    }
    Ok(resolved)
}

pub(super) fn resolve_stat_path(
    workspace: &Path,
    path: &str,
) -> Result<ResolvedPath, ToolResultEnvelope> {
    let (workspace, joined) = workspace_path(workspace, path)?;
    let resolved = match joined.canonicalize() {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = joined
                .parent()
                .and_then(|parent| parent.canonicalize().ok())
            {
                if !parent.starts_with(&workspace) {
                    return Err(outside_workspace(path));
                }
            }
            resolve_missing_path(&normalize_existing_components(joined), path)?
        }
        Err(error) => return Err(path_error(path, error)),
    };
    checked_resolved_path(workspace, resolved, path)
}

pub(super) fn resolve_remove_path(
    workspace: &Path,
    path: &str,
) -> Result<ResolvedPath, ToolResultEnvelope> {
    let (workspace, joined) = workspace_path(workspace, path)?;
    let metadata = fs::symlink_metadata(&joined).map_err(|error| path_error(path, error))?;
    if metadata.is_symlink() {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: symbolic link: {path}"),
            "remove_file requires the regular file's own path",
        ));
    }
    let resolved = joined
        .canonicalize()
        .map_err(|error| path_error(path, error))?;
    checked_resolved_path(workspace, resolved, path)
}

fn workspace_path(workspace: &Path, path: &str) -> Result<(PathBuf, PathBuf), ToolResultEnvelope> {
    let workspace = workspace.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            "failed: workspace not found",
            "workspace path does not exist",
        )
    })?;
    let joined = workspace.join(path);
    let lexical = normalize_existing_components(joined.clone());
    if let Ok(relative) = lexical.strip_prefix(&workspace) {
        guard_relative(&relative.to_string_lossy())?;
    }
    Ok((workspace, joined))
}

fn resolve_missing_path(path: &Path, requested: &str) -> Result<PathBuf, ToolResultEnvelope> {
    for ancestor in path.ancestors() {
        match ancestor.canonicalize() {
            Ok(canonical) => {
                let suffix = path.strip_prefix(ancestor).unwrap();
                return Ok(if suffix.as_os_str().is_empty() {
                    canonical
                } else {
                    canonical.join(suffix)
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if fs::symlink_metadata(ancestor).is_ok() {
                    return Err(path_error(requested, error));
                }
            }
            Err(error) => return Err(path_error(requested, error)),
        }
    }
    Err(ToolResultEnvelope::error(
        format!("failed: path not found: {requested}"),
        format!("path does not exist: {requested}"),
    ))
}

fn checked_resolved_path(
    workspace: PathBuf,
    resolved: PathBuf,
    requested: &str,
) -> Result<ResolvedPath, ToolResultEnvelope> {
    if !resolved.starts_with(&workspace) {
        return Err(outside_workspace(requested));
    }
    let relative = relative_path(&resolved, &workspace);
    guard_relative(&relative)?;
    Ok(ResolvedPath {
        workspace,
        path: resolved,
        relative,
    })
}

fn guard_relative(relative: &str) -> Result<(), ToolResultEnvelope> {
    if workspace_path_blocked(relative) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path is protected: {relative}"),
            format!("path is blocked by workspace guard: {relative}"),
        ));
    }
    Ok(())
}

pub(super) fn workspace_path_blocked(relative: &str) -> bool {
    #[cfg(windows)]
    let relative = &relative.to_ascii_lowercase();
    is_blocked_relative_path(relative)
}

fn outside_workspace(path: &str) -> ToolResultEnvelope {
    ToolResultEnvelope::blocked(
        format!("blocked: path outside workspace: {path}"),
        format!("path is outside the workspace: {path}"),
    )
}

fn path_error(path: &str, error: io::Error) -> ToolResultEnvelope {
    if error.kind() == io::ErrorKind::NotFound {
        return ToolResultEnvelope::error(
            format!("failed: path not found: {path}"),
            format!("path does not exist: {path}"),
        );
    }
    ToolResultEnvelope::error(
        format!("failed: {error}"),
        format!("cannot resolve path {path}: {error}"),
    )
}

pub(crate) fn normalize_existing_components(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(super) fn relative_path(path: &Path, workspace: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------- String helpers ----------

pub(super) fn join_bounded_strings(
    lines: &[String],
    max_chars: usize,
    truncation_message: &str,
) -> (String, bool) {
    let visible = bounded_lines(lines, max_chars);
    let mut model_content = visible.join("\n");
    let truncated = visible.len() < lines.len();
    if truncated {
        if !model_content.is_empty() {
            model_content.push('\n');
        }
        model_content.push_str(truncation_message);
    }
    (model_content, truncated)
}

pub(super) fn bounded_lines(lines: &[String], max_bytes: usize) -> &[String] {
    let mut bytes = 0;
    let count = lines
        .iter()
        .enumerate()
        .take_while(|(index, line)| {
            bytes += line.len() + usize::from(*index != 0);
            bytes <= max_bytes
        })
        .count();
    &lines[..count]
}

pub(super) fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

// ---------- Glob ----------

pub(super) fn glob_match(pattern: &str, path: &str) -> bool {
    let normalized = normalize_path_sep(path);
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("**/*") {
        return normalized.ends_with(suffix);
    }
    if let Some((prefix, suffix)) = pattern.split_once("/**/*") {
        return normalized.starts_with(&format!("{prefix}/")) && normalized.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return normalized == prefix || normalized.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return normalized
            .strip_prefix(&format!("{prefix}/"))
            .is_some_and(|rest| !rest.contains('/'));
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return normalized
            .rsplit('/')
            .next()
            .is_some_and(|name| name.ends_with(suffix));
    }
    normalized == pattern
}

// ---------- Parse helpers ----------

pub(super) fn optional_positive_usize(
    args: &Value,
    field: &str,
) -> Result<Option<usize>, ToolResultEnvelope> {
    let Some(value) = args.get(field) else {
        return Ok(None);
    };
    let Some(value) = value.as_u64() else {
        return Err(ToolResultEnvelope::error(
            format!("failed: {field} must be a positive integer"),
            format!("{field} must be a positive integer"),
        ));
    };
    if value == 0 {
        return Err(ToolResultEnvelope::error(
            format!("failed: {field} must be >= 1"),
            format!("{field} must be >= 1"),
        ));
    }
    usize::try_from(value).map(Some).map_err(|_| {
        ToolResultEnvelope::error(
            format!("failed: {field} is too large"),
            format!("{field} is too large"),
        )
    })
}

pub(super) fn requested_line_count(
    offset: usize,
    limit: Option<usize>,
    total_lines: usize,
) -> usize {
    if offset > total_lines {
        return 0;
    }
    let available = total_lines - offset + 1;
    limit.map_or(available, |limit| limit.min(available))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::address::ConversationAddress;
    use crate::event::{EventPayload, RollbackScope, StoredEvent};
    use crate::tool::builtin::test_helpers::stored_read_event;
    use snapshot::WriteSnapshot;

    fn find_write_snapshot(
        events: &[StoredEvent],
        canonical_path: &Path,
        require_full_file: bool,
        required_text: Option<&str>,
    ) -> Option<WriteSnapshot> {
        match super::find_write_snapshot(
            events,
            &ConversationAddress::MAIN,
            canonical_path,
            require_full_file,
            required_text,
            None,
        ) {
            WriteSnapshotLookup::Found(snapshot) => Some(snapshot),
            WriteSnapshotLookup::Rejected(_) => None,
        }
    }

    fn snapshot_event(id: u64, turn: u64, conversation: Option<&str>, path: &Path) -> StoredEvent {
        let mut event = stored_read_event(
            id,
            "1\talpha",
            serde_json::json!({
                "kind": "file_content",
                "canonical_path": path.to_string_lossy(),
                "content_hash": content_hash(b"alpha\n"),
                "raw_text": "alpha\n",
                "read_event_id": id,
                "start_line": 1,
                "line_count": 1,
                "total_lines": 1,
                "is_full_file_snapshot": true,
                "cached": false,
            }),
        );
        if let EventPayload::ToolResult {
            turn: event_turn,
            conversation: event_conversation,
            ..
        } = &mut event.payload
        {
            *event_turn = turn;
            *event_conversation = conversation.map(str::to_string);
        }
        event
    }

    #[cfg(unix)]
    #[test]
    fn write_atomically_rejects_existing_temp_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("safe.txt");
        let temp_path = dir.path().join("safe.txt.tmp");
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "outside\n").unwrap();

        std::os::unix::fs::symlink(outside.path(), &temp_path).unwrap();

        let result = write_atomically(&target, b"escaped\n");

        assert!(result.is_ok());
        assert_eq!("escaped\n", std::fs::read_to_string(&target).unwrap());
        assert_eq!(
            "outside\n",
            std::fs::read_to_string(outside.path()).unwrap()
        );
    }

    #[test]
    fn partial_snapshot_does_not_authorize_line_number_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let path = path.canonicalize().unwrap();
        let snapshot = stored_read_event(
            17,
            "1\talpha",
            serde_json::json!({
                "kind": "file_content",
                "canonical_path": path.to_string_lossy(),
                "content_hash": content_hash(b"alpha\nbeta\n"),
                "raw_text": "alpha\n",
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 1,
                "total_lines": 2,
                "is_full_file_snapshot": false,
                "cached": false,
            }),
        );

        assert!(find_write_snapshot(&[snapshot], &path, false, Some("1\talpha")).is_none());
    }

    #[test]
    fn partial_snapshot_does_not_authorize_text_past_visible_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let path = path.canonicalize().unwrap();
        let snapshot = stored_read_event(
            17,
            "1\talpha",
            serde_json::json!({
                "kind": "file_content",
                "canonical_path": path.to_string_lossy(),
                "content_hash": content_hash(b"alpha\nbeta\n"),
                "raw_text": "alpha\n",
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 1,
                "total_lines": 2,
                "is_full_file_snapshot": false,
                "cached": false,
            }),
        );

        assert!(find_write_snapshot(&[snapshot], &path, false, Some("alpha\nbeta")).is_none());
    }

    #[test]
    fn partial_snapshot_does_not_treat_lf_as_crlf_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\r\nbeta\r\n").unwrap();
        let path = path.canonicalize().unwrap();
        let snapshot = stored_read_event(
            17,
            "1\talpha\n2\tbeta",
            serde_json::json!({
                "kind": "file_content",
                "canonical_path": path.to_string_lossy(),
                "content_hash": content_hash(b"alpha\r\nbeta\r\n"),
                "raw_text": "alpha\r\nbeta\r\n",
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 2,
                "total_lines": 2,
                "is_full_file_snapshot": false,
                "cached": false,
            }),
        );

        assert!(find_write_snapshot(&[snapshot], &path, false, Some("\nbeta")).is_none());
    }

    #[test]
    fn write_snapshot_prefers_matching_hash_over_newer_stale_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let path = path.canonicalize().unwrap();
        let fresh = snapshot_event(17, 1, None, &path);
        let mut stale = snapshot_event(18, 2, None, &path);
        if let EventPayload::ToolResult {
            structured: Some(structured),
            ..
        } = &mut stale.payload
        {
            structured["content_hash"] = content_hash(b"stale\n").into();
        }
        let current_hash = content_hash(b"alpha\n");

        let result = super::find_write_snapshot(
            &[fresh, stale],
            &ConversationAddress::MAIN,
            &path,
            false,
            Some("alpha"),
            Some(&current_hash),
        );

        let WriteSnapshotLookup::Found(snapshot) = result else {
            panic!("expected a matching snapshot");
        };
        assert_eq!(snapshot.event_id, 17);
        assert_eq!(snapshot.content_hash, current_hash);
    }

    #[test]
    fn legacy_truncated_snapshot_does_not_authorize_raw_text_past_visible_line_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\nhidden tail\n").unwrap();
        let path = path.canonicalize().unwrap();
        let snapshot = stored_read_event(
            17,
            "1\talpha\n(Results are truncated. Use offset and limit to read a smaller range.)",
            serde_json::json!({
                "kind": "file_content",
                "canonical_path": path.to_string_lossy(),
                "content_hash": content_hash(b"alpha\nhidden tail\n"),
                "raw_text": "alpha\nhidden tail\n",
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 1,
                "total_lines": 2,
                "is_full_file_snapshot": false,
                "cached": false,
            }),
        );

        assert!(
            find_write_snapshot(std::slice::from_ref(&snapshot), &path, false, Some("alpha"))
                .is_some()
        );
        assert!(find_write_snapshot(&[snapshot], &path, false, Some("hidden tail")).is_none());
    }

    #[test]
    fn snapshot_from_another_conversation_cannot_authorize_main_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let path = path.canonicalize().unwrap();
        let snapshot = snapshot_event(1, 1, Some("review"), &path);

        assert!(find_write_snapshot(&[snapshot], &path, false, Some("alpha")).is_none());
    }

    #[test]
    fn rolled_back_snapshot_cannot_authorize_main_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let path = path.canonicalize().unwrap();
        let snapshot = snapshot_event(1, 2, None, &path);
        let rollback = StoredEvent {
            id: 2,
            payload: EventPayload::ConversationRollback {
                ts: "ts".to_string(),
                conversation: "main".to_string(),
                to_turn: 2,
                to_event_id: 1,
                scope: RollbackScope::ConversationOnly,
            },
        };

        assert!(find_write_snapshot(&[snapshot, rollback], &path, false, Some("alpha")).is_none());
    }

    #[test]
    fn compacted_snapshot_cannot_authorize_main_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let path = path.canonicalize().unwrap();
        let snapshot = snapshot_event(1, 1, None, &path);
        let handoff = StoredEvent {
            id: 2,
            payload: EventPayload::Handoff {
                turn: 1,
                ts: "ts".to_string(),
                request_id: "request_2".to_string(),
                summary: "continue".to_string(),
                keep_turns: 0,
            },
        };

        assert!(find_write_snapshot(&[snapshot, handoff], &path, false, Some("alpha")).is_none());
    }

    #[test]
    fn current_main_snapshot_still_authorizes_main_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let path = path.canonicalize().unwrap();
        let implicit_main = snapshot_event(1, 1, None, &path);
        let explicit_main = snapshot_event(2, 1, Some("main"), &path);

        let snapshot =
            find_write_snapshot(&[implicit_main, explicit_main], &path, false, Some("alpha"))
                .unwrap();

        assert_eq!(snapshot.event_id, 2);
    }

    #[test]
    fn executor_observation_is_not_model_visible_edit_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observed.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let path = path.canonicalize().unwrap();
        let mut read = snapshot_event(1, 1, None, &path);
        if let EventPayload::ToolResult {
            structured: Some(structured),
            ..
        } = &mut read.payload
        {
            structured["read_by"] = "executor".into();
        }

        assert!(find_write_snapshot(&[read], &path, false, Some("alpha")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn authorized_target_resolves_in_workspace_aliases_and_rejects_escape() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let original = workspace.path().join("original.txt");
        std::fs::write(&original, b"original").unwrap();
        std::os::unix::fs::symlink(&original, workspace.path().join("alias.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("outside")).unwrap();

        let alias = serde_json::json!({"path":"alias.txt"});
        let prepared = prepare_workspace_path("read_file", &alias, workspace.path()).unwrap();
        assert_eq!(Some(original.canonicalize().unwrap()), prepared);
        let rejected = prepare_workspace_path("remove_file", &alias, workspace.path()).unwrap_err();
        assert_eq!("blocked", rejected.status);

        for tool in ["stat_file", "write_file"] {
            let outside = serde_json::json!({"path":"outside/uncreated/deeper.txt"});
            let result = prepare_workspace_path(tool, &outside, workspace.path()).unwrap_err();
            assert_eq!("blocked", result.status);
        }
    }
}
