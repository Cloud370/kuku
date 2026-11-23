use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::tool::ToolResultEnvelope;

use super::common::{
    ensure_active, file_read_error, file_version, required_path, resolve_stat_path,
    FileCancellation,
};

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    File,
    Directory,
    Other,
    Missing,
}

pub(crate) fn stat_file(
    args: &Value,
    workspace: &Path,
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    let path = match required_path("stat_file", args) {
        Ok(path) => path,
        Err(result) => return result,
    };
    if let Err(error) = ensure_active(cancelled) {
        return file_read_error(Path::new(path), error);
    }
    let resolved = match resolve_stat_path(workspace, path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    let include_version = match args.get("version") {
        Some(Value::Bool(version)) => *version,
        None => false,
        Some(_) => {
            return ToolResultEnvelope::error(
                "failed: version must be a boolean",
                "stat_file version must be true or false",
            )
        }
    };
    let relative = if resolved.relative.is_empty() {
        "."
    } else {
        &resolved.relative
    };
    let (kind, size, hash) = match fs::symlink_metadata(&resolved.path) {
        Ok(metadata) if metadata.is_file() && include_version => {
            match file_version(&resolved.path, scan_bytes, cancelled) {
                Ok((hash, metadata)) => (EntryKind::File, Some(metadata.len()), Some(hash)),
                Err(error) => return file_read_error(Path::new(relative), error),
            }
        }
        Ok(metadata) if metadata.is_file() => (EntryKind::File, Some(metadata.len()), None),
        Ok(metadata) if metadata.is_dir() => (EntryKind::Directory, None, None),
        Ok(_) => (EntryKind::Other, None, None),
        Err(error) if error.kind() == ErrorKind::NotFound => (EntryKind::Missing, None, None),
        Err(error) => return file_read_error(Path::new(relative), error),
    };
    let summary = match kind {
        EntryKind::File => format!("file {relative}, {} bytes", size.unwrap()),
        EntryKind::Directory => format!("directory {relative}"),
        EntryKind::Other => format!("special entry {relative}"),
        EntryKind::Missing => format!("path {relative} is missing"),
    };
    let mut structured = serde_json::json!({
        "kind": "file_stat",
        "path": relative,
        "canonical_path": resolved.path.to_string_lossy(),
        "entry_kind": kind,
        "exists": !matches!(kind, EntryKind::Missing),
        "size_bytes": size,
    });
    let model_content = if let Some(hash) = hash {
        let details = format!("{summary}\ncontent_hash: {hash}");
        structured["content_hash"] = hash.into();
        details
    } else {
        summary.clone()
    };
    ToolResultEnvelope::ok(summary, model_content, structured)
}
