use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Result;
use crate::event::EventPayload;
use crate::execution::{FileCommitState, FileMutationFacts, FileMutationKind};

const PREVIEW_BYTES: usize = 8 * 1024;
const CHUNK_BYTES: usize = 64 * 1024;

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

    pub(crate) fn update_preview(&mut self, delta: &Self) -> Result<bool> {
        let mut previous = std::mem::take(&mut self.0);
        let mut used = 0;
        let mut omitted = false;
        let update = delta
            .0
            .iter()
            .map(|(path, change)| {
                let mut combined = previous.remove(path).unwrap_or_default();
                combined.add(change);
                (path.clone(), combined)
            })
            .collect::<Vec<_>>();
        for (path, change) in update.into_iter().chain(previous) {
            let bytes = serde_json::to_vec(&(&path, &change))?.len() + 1;
            if bytes <= PREVIEW_BYTES - used {
                used += bytes;
                self.0.insert(path, change);
            } else {
                omitted = true;
            }
        }
        Ok(omitted)
    }

    pub(crate) fn append_events(
        &self,
        events: &mut Vec<EventPayload>,
        agent: &str,
        segment: u64,
        ts: &str,
    ) -> Result<()> {
        let mut entries = Vec::new();
        let mut bytes = 0;
        let event = |files: Vec<Value>| {
            EventPayload::Unknown(json!({
                "kind":"runtime.checkpoint_file_changes", "ts":ts, "agent":agent,
                "segment":segment, "files":files,
            }))
        };
        for (path, change) in &self.0 {
            let entry = serde_json::to_value((path, change))?;
            let size = serde_json::to_vec(&entry)?.len() + 1;
            if !entries.is_empty() && bytes + size > CHUNK_BYTES {
                events.push(event(std::mem::take(&mut entries)));
                bytes = 0;
            }
            entries.push(entry);
            bytes += size;
        }
        if !entries.is_empty() {
            events.push(event(entries));
        }
        Ok(())
    }

    pub(crate) fn append_to(&self, summary: &str, mut omitted: bool) -> String {
        if self.0.is_empty() && !omitted {
            return summary.to_owned();
        }
        let mut output = String::from(summary);
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("## Recorded file changes\n\n");
        let start = output.len();
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
            let line = format!("- {path} · {}\n", details.join(" · "));
            if output.len() - start + line.len() <= PREVIEW_BYTES {
                output.push_str(&line);
            } else {
                omitted = true;
            }
        }
        if omitted {
            writeln!(
                output,
                "Additional file changes are recorded in the Session journal."
            )
            .expect("writing to a string");
        }
        output.truncate(output.trim_end().len());
        output
    }
}

impl FileChange {
    fn add(&mut self, later: &Self) {
        self.added = self.added.saturating_add(later.added);
        self.removed = self.removed.saturating_add(later.removed);
        self.edits = self.edits.saturating_add(later.edits);
        self.edits_without_counts = self
            .edits_without_counts
            .saturating_add(later.edits_without_counts);
        self.writes = self.writes.saturating_add(later.writes);
        self.removals = self.removals.saturating_add(later.removals);
        if later.writes > 0 {
            self.last_write_lines = later.last_write_lines;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckpointFileChanges, FileChange, PREVIEW_BYTES};
    use crate::event::{EventPayload, EventStore};

    #[test]
    fn large_file_inventory_keeps_every_entry_in_bounded_journal_chunks() {
        let mut delta = CheckpointFileChanges::default();
        for index in 0..4_600 {
            let path = format!("docs/{index:05}/{}.md", "a".repeat(107));
            delta.0.insert(
                path,
                FileChange {
                    added: 1,
                    removed: 1,
                    edits: 1,
                    ..Default::default()
                },
            );
        }
        let mut preview = CheckpointFileChanges::default();
        let omitted = preview.update_preview(&delta).unwrap();
        assert!(omitted);
        let text = preview.append_to("Work completed.", omitted);
        assert!(text.contains("edited +1 −1 lines"));
        assert!(text.contains("Additional file changes"));
        assert!(text.len() < PREVIEW_BYTES + 256);

        let mut events = Vec::new();
        delta
            .append_events(&mut events, "main", 5, "2026-11-22T00:00:00Z")
            .unwrap();
        assert!(events.len() > 1);
        EventStore::validate_batch_records(6, &events).unwrap();
        let saved = events
            .iter()
            .map(|event| match event {
                EventPayload::Unknown(facts) => {
                    assert_eq!(5, facts["segment"]);
                    facts["files"].as_array().unwrap().len()
                }
                _ => panic!("expected a checkpoint file inventory"),
            })
            .sum::<usize>();
        assert_eq!(delta.0.len(), saved);
    }
}
