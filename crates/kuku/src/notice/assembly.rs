use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::Digest;

use crate::event::{EventPayload, StoredEvent};

use super::types::{
    ContextBudgetTier, ContextDriftEntry, ContextDriftStatus, Notice, NoticeKind, NoticeSeverity,
};

pub(crate) struct NoticeAssemblyInput<'a> {
    pub(crate) workspace: &'a Path,
    pub(crate) events: &'a [StoredEvent],
    pub(crate) context_budget_tier: ContextBudgetTier,
}

struct TrackedFileSnapshot {
    path: String,
    hash: String,
}

pub(crate) fn build_runtime_notices(input: NoticeAssemblyInput<'_>) -> Vec<Notice> {
    let mut notices = Vec::new();

    if let Some(notice) =
        build_context_drift_notice(input.workspace, input.events, input.context_budget_tier)
    {
        notices.push(notice);
    }

    notices
}

fn build_context_drift_notice(
    workspace: &Path,
    events: &[StoredEvent],
    tier: ContextBudgetTier,
) -> Option<Notice> {
    let tracked = rebuild_tracked_file_snapshots(events);
    if tracked.is_empty() {
        return None;
    }

    let mut entries = Vec::new();
    for snapshot in tracked.values() {
        let path = PathBuf::from(&snapshot.path);
        let label = path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match std::fs::read(&path) {
            Ok(current_bytes) => {
                let current_hash = content_hash_bytes(&current_bytes);
                if current_hash == snapshot.hash {
                    continue;
                }
                entries.push(ContextDriftEntry {
                    path: label,
                    status: ContextDriftStatus::Updated,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                entries.push(ContextDriftEntry {
                    path: label,
                    status: ContextDriftStatus::Deleted,
                });
            }
            Err(_) => continue,
        }
    }

    if entries.is_empty() {
        return None;
    }

    let max = max_context_drift_entries(tier);
    entries.truncate(max);

    Some(Notice {
        kind: NoticeKind::ContextDrift { entries },
        severity: NoticeSeverity::Info,
    })
}

fn max_context_drift_entries(tier: ContextBudgetTier) -> usize {
    match tier {
        ContextBudgetTier::Tight => 4,
        ContextBudgetTier::Normal => 12,
        ContextBudgetTier::Roomy => 32,
    }
}

fn rebuild_tracked_file_snapshots(events: &[StoredEvent]) -> BTreeMap<String, TrackedFileSnapshot> {
    let mut tracked = tracked_files_from_latest_model_request(events);

    for event in events {
        let EventPayload::ToolResult {
            status,
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            continue;
        };
        if status != "ok" {
            continue;
        }
        update_tracked_snapshot_from_tool_result(&mut tracked, structured);
    }

    tracked
}

fn tracked_files_from_latest_model_request(
    events: &[StoredEvent],
) -> BTreeMap<String, TrackedFileSnapshot> {
    let mut tracked = BTreeMap::new();
    let Some(provenance) = events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelRequest {
            provenance: Some(provenance),
            ..
        } => Some(provenance),
        _ => None,
    }) else {
        return tracked;
    };

    for source in provenance
        .get("project_instruction_sources")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(path), Some(hash)) = (
            source.get("path").and_then(serde_json::Value::as_str),
            source.get("hash").and_then(serde_json::Value::as_str),
        ) {
            tracked.insert(
                path.to_string(),
                TrackedFileSnapshot {
                    path: path.to_string(),
                    hash: hash.to_string(),
                },
            );
        }
    }
    for source in provenance
        .get("memory_sources")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(path), Some(hash)) = (
            source.get("path").and_then(serde_json::Value::as_str),
            source.get("hash").and_then(serde_json::Value::as_str),
        ) {
            tracked.insert(
                path.to_string(),
                TrackedFileSnapshot {
                    path: path.to_string(),
                    hash: hash.to_string(),
                },
            );
        }
    }

    tracked
}

fn update_tracked_snapshot_from_tool_result(
    tracked: &mut BTreeMap<String, TrackedFileSnapshot>,
    structured: &serde_json::Value,
) {
    let Some(kind) = structured.get("kind").and_then(serde_json::Value::as_str) else {
        return;
    };
    match kind {
        "file_content" => {
            let is_full = structured
                .get("is_full_file_snapshot")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !is_full {
                return;
            }
            let Some(path) = structured
                .get("canonical_path")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            let Some(hash) = structured
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            tracked.insert(
                path.to_string(),
                TrackedFileSnapshot {
                    path: path.to_string(),
                    hash: hash.to_string(),
                },
            );
        }
        "file_edit" | "file_write" | "memory_write" | "memory_forget" => {
            let Some(path) = structured
                .get("canonical_path")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            let Some(existing) = tracked.get_mut(path) else {
                return;
            };
            let Some(hash) = structured
                .get("content_hash_after")
                .or_else(|| structured.get("content_hash"))
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            existing.hash = hash.to_string();
        }
        _ => {}
    }
}

fn content_hash_bytes(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notice::render::render_notice_block;

    fn make_entry(index: usize) -> ContextDriftEntry {
        ContextDriftEntry {
            path: format!("file-{index}.md"),
            status: ContextDriftStatus::Updated,
        }
    }

    #[test]
    fn tight_budget_truncates_context_drift_entries() {
        let entries: Vec<ContextDriftEntry> = (0..10).map(make_entry).collect();
        let max = max_context_drift_entries(ContextBudgetTier::Tight);
        assert_eq!(max, 4);

        let mut truncated = entries;
        truncated.truncate(max);
        let notice = Notice {
            kind: NoticeKind::ContextDrift { entries: truncated },
            severity: NoticeSeverity::Info,
        };
        let rendered = render_notice_block(&notice);

        assert!(rendered.contains("Changed tracked files:"));
        assert!(rendered.contains("file-0.md"));
        assert!(rendered.contains("file-3.md"));
        assert!(!rendered.contains("file-4.md"));
        assert!(!rendered.contains("current preview:"));
        assert!(!rendered.contains("line 17"));
    }

    #[test]
    fn normal_budget_allows_more_entries_than_tight() {
        assert!(
            max_context_drift_entries(ContextBudgetTier::Normal)
                > max_context_drift_entries(ContextBudgetTier::Tight)
        );
        assert!(
            max_context_drift_entries(ContextBudgetTier::Roomy)
                > max_context_drift_entries(ContextBudgetTier::Normal)
        );
    }
}
