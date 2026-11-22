use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::execution::{FileCommitState, FileMutationFacts, FileMutationKind};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct CheckpointFileChanges(BTreeMap<String, FileChange>);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileChange {
    added: u64,
    removed: u64,
    edits: u64,
    edits_without_counts: u64,
    writes: u64,
    last_write_lines: Option<u64>,
    removals: u64,
}

impl CheckpointFileChanges {
    pub(crate) fn record_result(&mut self, status: &str, value: &Value) {
        if value["kind"] == "workspace_script_result" {
            if let Some(actions) = value["actions"].as_array() {
                for action in actions.iter().filter(|action| action["status"] == "ok") {
                    self.observe(&action["result"]);
                }
            }
        } else if status == "ok" {
            self.observe(value);
        }
    }

    fn observe(&mut self, value: &Value) {
        if !matches!(
            value["kind"].as_str(),
            Some("file_edit" | "file_write" | "file_remove")
        ) {
            return;
        }
        let Ok(facts) = FileMutationFacts::from_structured(value) else {
            return;
        };
        if facts.path.is_empty()
            || facts.commit_state != FileCommitState::Committed
            || (facts.content_hash_before.is_some()
                && facts.content_hash_before == facts.content_hash_after)
        {
            return;
        }
        let change = self.0.entry(facts.path).or_default();
        match facts.kind {
            FileMutationKind::FileEdit => {
                change.edits = change.edits.saturating_add(1);
                let stats = &value["change"];
                if let (Some(added), Some(removed)) = (
                    stats["added_lines"].as_u64(),
                    stats["removed_lines"].as_u64(),
                ) {
                    change.added = change.added.saturating_add(added);
                    change.removed = change.removed.saturating_add(removed);
                } else {
                    change.edits_without_counts = change.edits_without_counts.saturating_add(1);
                }
            }
            FileMutationKind::FileWrite => {
                change.writes = change.writes.saturating_add(1);
                change.last_write_lines = value["line_count"].as_u64();
            }
            FileMutationKind::FileRemove => change.removals = change.removals.saturating_add(1),
        }
    }

    pub(crate) fn append_to(&self, summary: &str) -> String {
        if self.0.is_empty() {
            return summary.to_owned();
        }
        let mut output = String::from(summary);
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("## Recorded file changes\n\n");
        for (path, change) in &self.0 {
            let mut details = Vec::with_capacity(4);
            if change.edits > change.edits_without_counts {
                details.push(format!(
                    "edited +{} −{} lines",
                    change.added, change.removed
                ));
            }
            if change.edits_without_counts > 0 {
                details.push(format!(
                    "{} edits without line counts",
                    change.edits_without_counts
                ));
            }
            if change.writes > 0 {
                details.push(match change.last_write_lines {
                    Some(1) => "last write 1 line".into(),
                    Some(lines) => format!("last write {lines} lines"),
                    None => "written, line count unavailable".into(),
                });
            }
            if change.removals > 0 {
                details.push("removed in a recorded operation".into());
            }
            let path = serde_json::to_string(path).expect("a path string serializes");
            writeln!(output, "- {path} · {}", details.join(" · ")).expect("writing to a string");
        }
        output.truncate(output.trim_end().len());
        output
    }
}
