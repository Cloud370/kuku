use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::tool::builtin::common::content_hash;
use crate::{Error, Result};

use super::message_rewind::{file_snapshot, is_created_file_write};
use super::{
    event_owner, event_turn_owners, message_rewind_comparison_path, message_rewind_path,
    reject_symlink_components, revertable_path, FileRestore, RevertPlan, RewindVisibility,
};

/// Determine workspace changes needed to rewind a message to its visible history boundary.
pub fn compute_message_rewind_plan(
    events: &[StoredEvent],
    conversation: &str,
    target_event_id: u64,
    workspace: &Path,
) -> Result<RevertPlan> {
    let position = events
        .iter()
        .position(|event| event.id == target_event_id)
        .ok_or_else(|| Error::InvalidArgument("rewind target is no longer available".into()))?;
    let visibility = RewindVisibility::new(events);
    let owners = event_turn_owners(events);
    let mut plan = FilePlan::new(workspace)?;
    for event in &events[position..] {
        plan.observe_change(event, conversation, &visibility, &owners)?;
    }
    plan.check_conflicts()?;
    let snapshot_end = plan.snapshot_end();
    for event in events.iter().take_while(|event| event.id < snapshot_end) {
        plan.observe_snapshot(event, &visibility, &owners);
    }
    plan.finish()
}

struct FilePlan {
    workspace: PathBuf,
    modified: HashMap<PathBuf, FirstMutation>,
    other_changes: BTreeSet<String>,
}

struct FirstMutation {
    id: u64,
    created: bool,
    before: Option<String>,
}

/// The proposed effect on one workspace file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileRewindChange {
    /// Workspace-relative path.
    pub path: String,
    /// Whether the restore would remove the file.
    pub remove: bool,
    /// Lines added and removed when applying the restore, when available.
    pub lines: Option<(usize, usize)>,
}

/// A preview of the file changes that an explicit Turn rewind can apply.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileRewindPreview {
    /// Identity of the exact approved file plan.
    pub fingerprint: String,
    /// Verified file changes.
    pub changes: Vec<FileRewindChange>,
    /// Paths whose previous or current version could not be verified.
    pub skipped: Vec<String>,
}

pub(crate) struct VerifiedFileRewind {
    pub(crate) plan: RevertPlan,
    pub(crate) expected: BTreeMap<PathBuf, String>,
    pub(crate) preview: FileRewindPreview,
}

struct FileVersions {
    before: Option<String>,
    after: Option<String>,
    bytes: Option<u64>,
    verified: bool,
}

pub(crate) fn plan_file_rewind(
    path: &Path,
    workspace: &Path,
    address: &str,
    target: u64,
) -> Result<VerifiedFileRewind> {
    let (plan, versions) = EventStore::with_file_history(path, |mut history| {
        let visibility = RewindVisibility::new(&history.take_controls());
        if visibility.hidden_start(address, target).is_some() {
            return Err(Error::InvalidArgument(
                "Files can be restored only when going back within the current history".into(),
            ));
        }
        let owners = history.take_owners();
        let mut plan = FilePlan::new(workspace)?;
        let mut versions = HashMap::<PathBuf, FileVersions>::new();
        history.visit(target, u64::MAX, |event| {
            let EventPayload::ToolResult {
                structured: Some(facts),
                ..
            } = &event.payload
            else {
                return Ok(());
            };
            if !matches!(facts["kind"].as_str(), Some("file_write" | "file_edit")) {
                return Ok(());
            }
            if let Some(path) = plan.observe_change(&event, address, &visibility, &owners)? {
                let before = facts["content_hash_before"].as_str().map(str::to_owned);
                let after = facts["content_hash_after"].as_str().map(str::to_owned);
                let verified = facts["commit_state"] == "committed" && after.is_some();
                let file = versions.entry(path).or_insert_with(|| FileVersions {
                    before: before.clone(),
                    after: before.clone(),
                    bytes: None,
                    verified: true,
                });
                file.verified &= verified && file.after == before;
                file.after = after;
                file.bytes = facts["bytes_written"].as_u64();
            }
            Ok(())
        })?;
        plan.check_conflicts()?;
        history.visit(0, plan.snapshot_end(), |event| {
            plan.observe_snapshot(&event, &visibility, &owners);
            Ok(())
        })?;
        Ok((plan, versions))
    })?;

    let mut stored = RevertPlan {
        restores: Vec::new(),
        deletes: Vec::new(),
        unrecoverable: Vec::new(),
        sensitive_files: Vec::new(),
    };
    let mut expected = BTreeMap::new();
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    let mut identities = Vec::new();
    for (path, mutation) in BTreeMap::from_iter(plan.modified) {
        reject_symlink_components(&plan.workspace, &path)?;
        let name = path
            .strip_prefix(&plan.workspace)
            .unwrap_or(&path)
            .display()
            .to_string();
        let version = &versions[&path];
        let current = version
            .bytes
            .and_then(|bytes| read_observed_file(&path, bytes).ok());
        let current_hash = current.as_ref().map(|text| content_hash(text.as_bytes()));
        let previous_hash = mutation
            .before
            .as_ref()
            .map(|text| content_hash(text.as_bytes()));
        if !version.verified
            || current_hash.is_none()
            || current_hash != version.after
            || previous_hash != version.before
            || (mutation.before.is_none() && !mutation.created)
        {
            skipped.push(name);
            continue;
        }
        let current = current.unwrap();
        if mutation.before.as_ref() == Some(&current) {
            continue;
        }
        let new_hash = current_hash.unwrap();
        let remove = mutation.before.is_none();
        let lines = match mutation.before {
            Some(old_content) => {
                let lines = crate::tool::builtin::line_change_counts(&current, &old_content);
                stored.restores.push(FileRestore {
                    path: path.clone(),
                    old_hash: previous_hash.clone().unwrap(),
                    old_content,
                    new_content_on_plan: current,
                });
                lines
            }
            None => {
                stored.deletes.push(path.clone());
                Some((0, current.lines().count()))
            }
        };
        identities.push((name.clone(), previous_hash, new_hash.clone()));
        changes.push(FileRewindChange {
            path: name,
            remove,
            lines,
        });
        expected.insert(path, new_hash);
    }
    let fingerprint = content_hash(&serde_json::to_vec(&(address, target, identities))?);
    Ok(VerifiedFileRewind {
        plan: stored,
        expected,
        preview: FileRewindPreview {
            fingerprint,
            changes,
            skipped,
        },
    })
}

fn read_observed_file(path: &Path, bytes: u64) -> std::io::Result<String> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() != bytes {
        return Err(std::io::Error::other(
            "file length changed since the recorded edit",
        ));
    }
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(bytes.saturating_add(1))
        .read_to_string(&mut text)?;
    Ok(text)
}

impl FilePlan {
    fn new(workspace: &Path) -> Result<Self> {
        Ok(Self {
            workspace: workspace.canonicalize()?,
            modified: HashMap::new(),
            other_changes: BTreeSet::new(),
        })
    }

    fn observe_change(
        &mut self,
        event: &StoredEvent,
        conversation: &str,
        visibility: &RewindVisibility,
        owners: &HashMap<u64, String>,
    ) -> Result<Option<PathBuf>> {
        let Some(path) = revertable_path(event) else {
            return Ok(None);
        };
        let owner = event_owner(event, owners);
        if owner != Some(conversation) {
            self.other_changes.insert(path);
        } else if visibility.includes(event, owner) {
            let path = message_rewind_path(&self.workspace, &path)?;
            self.modified
                .entry(path.clone())
                .or_insert_with(|| FirstMutation {
                    id: event.id,
                    created: is_created_file_write(event),
                    before: None,
                });
            return Ok(Some(path));
        }
        Ok(None)
    }

    fn check_conflicts(&self) -> Result<()> {
        for path in &self.other_changes {
            if self
                .modified
                .contains_key(&message_rewind_comparison_path(path)?)
            {
                return Err(Error::InvalidArgument(format!(
                    "cannot restore {path} because another Agent recorded a later change"
                )));
            }
        }
        Ok(())
    }

    fn snapshot_end(&self) -> u64 {
        self.modified
            .values()
            .filter(|change| !change.created)
            .map(|change| change.id)
            .max()
            .unwrap_or_default()
    }

    fn observe_snapshot(
        &mut self,
        event: &StoredEvent,
        visibility: &RewindVisibility,
        owners: &HashMap<u64, String>,
    ) {
        if let Some((path, content)) = file_snapshot(event, &self.workspace) {
            if let Some(change) = self.modified.get_mut(&path).filter(|change| {
                !change.created
                    && event.id < change.id
                    && visibility.includes(event, event_owner(event, owners))
            }) {
                change.before = Some(content.to_owned());
            }
        }
    }

    fn finish(self) -> Result<RevertPlan> {
        let mut plan = RevertPlan {
            restores: Vec::new(),
            deletes: Vec::new(),
            unrecoverable: Vec::new(),
            sensitive_files: Vec::new(),
        };
        for (path, change) in self.modified {
            reject_symlink_components(&self.workspace, &path)?;
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            if crate::util::path::is_sensitive_file_name(&file_name) {
                plan.sensitive_files.push(path.clone());
            }
            if let Some(old_content) = change.before {
                let new_content = std::fs::read_to_string(&path)?;
                if old_content != new_content {
                    plan.restores.push(FileRestore {
                        path,
                        old_hash: content_hash(old_content.as_bytes()),
                        old_content,
                        new_content_on_plan: new_content,
                    });
                }
            } else if change.created {
                if path.exists() {
                    plan.deletes.push(path);
                }
            } else {
                plan.unrecoverable.push(path);
            }
        }
        Ok(plan)
    }
}
