use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::event::{EventPayload, StoredEvent};
use crate::tool::ToolResultEnvelope;

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

pub(super) fn normalize_existing_components(path: PathBuf) -> PathBuf {
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

pub(super) fn is_blocked_relative_path(path: &str) -> bool {
    path.split('/').any(|part| part == ".git" || part == ".ssh")
        || path.rsplit('/').next().is_some_and(is_sensitive_file_name)
}

fn is_sensitive_file_name(name: &str) -> bool {
    matches!(
        name,
        ".env"
            | "credentials.json"
            | "credentials.toml"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
    ) || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
}

// ---------- Content helpers ----------

pub(super) fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

pub(super) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("kuku")
    ));
    fs::write(&temp_path, bytes)?;
    fs::rename(temp_path, path)
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
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("**/*") {
        return path.ends_with(suffix);
    }
    if let Some((prefix, suffix)) = pattern.split_once("/**/*") {
        return path.starts_with(&format!("{prefix}/")) && path.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return path
            .strip_prefix(&format!("{prefix}/"))
            .is_some_and(|rest| !rest.contains('/'));
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.ends_with(suffix));
    }
    path == pattern
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
    canonical_path: &Path,
    content_hash: &str,
    start_line: usize,
    line_count: usize,
) -> Option<ReadSnapshot> {
    let canonical_path = canonical_path.to_string_lossy();
    events.iter().rev().find_map(|event| {
        let EventPayload::ToolResult {
            status,
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            return None;
        };
        if status != "ok" || structured["kind"] != "file_content" || structured["cached"] == true {
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
    canonical_path: &Path,
    require_full_file: bool,
    required_text: Option<&str>,
) -> Option<WriteSnapshot> {
    let canonical_path = canonical_path.to_string_lossy();
    events.iter().rev().find_map(|event| {
        let EventPayload::ToolResult {
            status,
            model_content,
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            return None;
        };
        if status != "ok" || structured["kind"] != "file_content" || structured["cached"] == true {
            return None;
        }
        if structured["canonical_path"].as_str()? != canonical_path {
            return None;
        }
        let is_full_file_snapshot = structured["is_full_file_snapshot"]
            .as_bool()
            .unwrap_or(false);
        if require_full_file && !is_full_file_snapshot {
            return None;
        }
        if !is_full_file_snapshot && required_text.is_some_and(|text| !model_content.contains(text))
        {
            return None;
        }
        Some(WriteSnapshot {
            event_id: structured["read_event_id"].as_u64().unwrap_or(event.id),
            content_hash: structured["content_hash"].as_str()?.to_string(),
        })
    })
}
