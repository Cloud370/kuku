use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::event::{EventPayload, StoredEvent};
use crate::tool::{ToolErrorReason, ToolResultEnvelope};
use crate::util::path::{is_blocked_relative_path, normalize_path_sep};

// ---------- Types ----------

pub(super) struct ResolvedPath {
    pub(super) workspace: PathBuf,
    pub(super) path: PathBuf,
    pub(super) relative: String,
}

pub(super) struct ReadSnapshot {
    pub(super) event_id: u64,
    pub(super) start_line: usize,
    pub(super) line_count: usize,
    pub(super) is_full_file_snapshot: bool,
}

pub(super) struct WriteSnapshot {
    pub(super) event_id: u64,
    pub(super) content_hash: String,
}

pub(super) enum WriteSnapshotLookup {
    Found(WriteSnapshot),
    Rejected(ToolErrorReason),
}

// ---------- Path resolution ----------

pub(super) fn resolve_path(
    workspace: &Path,
    path: &str,
) -> Result<ResolvedPath, ToolResultEnvelope> {
    let workspace = workspace.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            "failed: workspace not found",
            "workspace path does not exist",
        )
    })?;
    let candidate = Path::new(path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    let resolved = joined.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            format!("failed: path not found: {path}"),
            format!("path does not exist: {path}"),
        )
    })?;

    if !resolved.starts_with(&workspace) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path outside workspace: {path}"),
            format!("path is outside the workspace: {path}"),
        ));
    }

    let relative = relative_path(&resolved, &workspace);
    if is_blocked_relative_path(&relative) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path is not readable: {relative}"),
            format!("path is blocked by read guard: {relative}"),
        ));
    }

    Ok(ResolvedPath {
        workspace,
        path: resolved,
        relative,
    })
}

pub(super) fn resolve_write_path(
    workspace: &Path,
    path: &str,
) -> Result<ResolvedPath, ToolResultEnvelope> {
    match resolve_path(workspace, path) {
        Ok(existing) => return Ok(existing),
        Err(result) if result.status == "blocked" => return Err(result),
        Err(_) => {}
    }

    let workspace = workspace.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            "failed: workspace not found",
            "workspace path does not exist",
        )
    })?;
    let candidate = Path::new(path);
    let joined = normalize_existing_components(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    });
    let Some(parent) = joined.parent() else {
        return Err(ToolResultEnvelope::error(
            format!("failed: missing parent for {path}"),
            format!("path has no parent directory: {path}"),
        ));
    };
    let parent = parent.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            format!("failed: parent path not found: {path}"),
            format!("parent directory does not exist: {path}"),
        )
    })?;
    if !parent.starts_with(&workspace) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path outside workspace: {path}"),
            format!("path is outside the workspace: {path}"),
        ));
    }
    let file_name = joined.file_name().ok_or_else(|| {
        ToolResultEnvelope::error(
            format!("failed: missing file name: {path}"),
            format!("path has no file name: {path}"),
        )
    })?;
    let resolved = parent.join(file_name);
    let relative = relative_path(&resolved, &workspace);
    if is_blocked_relative_path(&relative) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path is not writable: {relative}"),
            format!("path is blocked by write guard: {relative}"),
        ));
    }
    Ok(ResolvedPath {
        workspace,
        path: resolved,
        relative,
    })
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

pub(super) fn is_default_excluded_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | "__pycache__" | ".venv" | "venv" | "dist" | "build"
    )
}

// ---------- File I/O helpers ----------

pub(super) fn read_file_as_utf8(path: &Path) -> Result<(String, Vec<u8>), ToolResultEnvelope> {
    let bytes = fs::read(path).map_err(|error| {
        ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error reading file: {}", path.display()),
        )
    })?;
    let content = String::from_utf8(bytes.clone()).map_err(|_| {
        ToolResultEnvelope::error(
            format!("failed: file is not valid UTF-8: {}", path.display()),
            format!("file is not valid UTF-8: {}", path.display()),
        )
    })?;
    Ok((content, bytes))
}

pub(super) fn require_brief(tool_name: &str, args: &Value) -> Result<String, ToolResultEnvelope> {
    let Some(brief) = args.get("brief").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing brief",
            format!("{tool_name} requires brief"),
        ));
    };
    if brief.trim().is_empty() {
        return Err(ToolResultEnvelope::error(
            "failed: brief is empty",
            "brief must not be empty",
        ));
    }
    Ok(brief.to_string())
}

// ---------- Content helpers ----------

pub(crate) fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        )
    })?;
    let mut builder = tempfile::Builder::new();
    let prefix = temp_file_prefix(path);
    builder.prefix(&prefix);
    let mut temp_file = builder.tempfile_in(parent)?;
    temp_file.write_all(bytes)?;
    temp_file.flush()?;
    temp_file
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.error)
}

fn temp_file_prefix(path: &Path) -> OsString {
    let mut prefix = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| OsString::from("kuku"));
    prefix.push(".");
    prefix
}

// ---------- String helpers ----------

pub(super) fn join_bounded_strings(
    lines: &[String],
    max_chars: usize,
    truncation_message: &str,
) -> (String, bool) {
    let mut model_content = String::new();
    let mut truncated = false;
    for line in lines {
        let next_len = model_content.len() + line.len() + usize::from(!model_content.is_empty());
        if next_len > max_chars {
            truncated = true;
            break;
        }
        if !model_content.is_empty() {
            model_content.push('\n');
        }
        model_content.push_str(line);
    }
    if truncated {
        if !model_content.is_empty() {
            model_content.push('\n');
        }
        model_content.push_str(truncation_message);
    }
    (model_content, truncated)
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

// ---------- Snapshot finders ----------

pub(super) fn find_covering_read(
    events: &[StoredEvent],
    conversation: &crate::conversation::address::ConversationAddress,
    canonical_path: &Path,
    content_hash: &str,
    start_line: usize,
    line_count: usize,
) -> Option<ReadSnapshot> {
    let canonical_path = canonical_path.to_string_lossy();
    crate::context::replay::effective_snapshot_events(events, conversation)
        .into_iter()
        .rev()
        .find_map(|event| {
            let EventPayload::ToolResult {
                status,
                structured: Some(structured),
                ..
            } = &event.payload
            else {
                return None;
            };
            if status != "ok"
                || structured["kind"] != "file_content"
                || structured["cached"] == true
            {
                return None;
            }
            if structured["canonical_path"].as_str()? != canonical_path
                || structured["content_hash"].as_str()? != content_hash
            {
                return None;
            }
            let snapshot = ReadSnapshot {
                event_id: structured["read_event_id"].as_u64().unwrap_or(event.id),
                start_line: structured["start_line"].as_u64()? as usize,
                line_count: structured["line_count"].as_u64()? as usize,
                is_full_file_snapshot: structured["is_full_file_snapshot"]
                    .as_bool()
                    .unwrap_or(false),
            };
            if snapshot.covers(start_line, line_count) {
                Some(snapshot)
            } else {
                None
            }
        })
}

impl ReadSnapshot {
    fn covers(&self, start_line: usize, line_count: usize) -> bool {
        self.is_full_file_snapshot
            || (self.start_line == start_line && self.line_count == line_count)
    }
}

pub(super) fn find_write_snapshot(
    events: &[StoredEvent],
    conversation: &crate::conversation::address::ConversationAddress,
    canonical_path: &Path,
    require_full_file: bool,
    required_text: Option<&str>,
) -> WriteSnapshotLookup {
    let canonical_path = canonical_path.to_string_lossy();
    let mut saw_path_snapshot = false;
    for event in crate::context::replay::effective_snapshot_events(events, conversation)
        .into_iter()
        .rev()
    {
        let EventPayload::ToolResult {
            status,
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            continue;
        };
        if status != "ok" || structured["kind"] != "file_content" || structured["cached"] == true {
            continue;
        }
        if structured["canonical_path"].as_str() != Some(canonical_path.as_ref()) {
            continue;
        }
        let Some(content_hash) = structured["content_hash"].as_str() else {
            continue;
        };
        saw_path_snapshot = true;
        let is_full_file_snapshot = structured["is_full_file_snapshot"]
            .as_bool()
            .unwrap_or(false);
        if require_full_file && !is_full_file_snapshot {
            continue;
        }
        if !is_full_file_snapshot
            && required_text.is_some_and(|text| {
                !visible_snapshot_raw_text(structured)
                    .is_some_and(|raw_text| raw_text.contains(text))
            })
        {
            continue;
        }
        return WriteSnapshotLookup::Found(WriteSnapshot {
            event_id: structured["read_event_id"].as_u64().unwrap_or(event.id),
            content_hash: content_hash.to_string(),
        });
    }
    if require_full_file {
        WriteSnapshotLookup::Rejected(ToolErrorReason::FullSnapshotRequired)
    } else if saw_path_snapshot && required_text.is_some() {
        WriteSnapshotLookup::Rejected(ToolErrorReason::OldTextNotVisible)
    } else {
        WriteSnapshotLookup::Rejected(ToolErrorReason::SnapshotRequired)
    }
}

fn visible_snapshot_raw_text(structured: &Value) -> Option<&str> {
    let raw_text = structured["raw_text"].as_str()?;
    let line_count = structured["line_count"].as_u64()? as usize;
    let visible_len = raw_text
        .split_inclusive('\n')
        .take(line_count)
        .map(str::len)
        .sum();
    raw_text.get(..visible_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::address::ConversationAddress;
    use crate::event::RollbackScope;
    use crate::tool::builtin::test_helpers::stored_read_event;

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

        assert!(find_write_snapshot(&[snapshot.clone()], &path, false, Some("alpha")).is_some());
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
}
