use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::event::{EventStore, StoredEvent};
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

pub(crate) fn plan_journal_rewind(
    store: &EventStore,
    visibility: &RewindVisibility<'_>,
    conversation: &str,
    target_event_id: u64,
    workspace: &Path,
) -> Result<RevertPlan> {
    let owners = store.last_turn_owners();
    let mut plan = FilePlan::new(workspace)?;
    store.visit_file_results(target_event_id.saturating_sub(1), u64::MAX, |event| {
        plan.observe_change(&event, conversation, visibility, &owners)
    })?;
    plan.check_conflicts()?;
    store.visit_file_results(0, plan.snapshot_end(), |event| {
        plan.observe_snapshot(&event, visibility, &owners);
        Ok(())
    })?;
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
        visibility: &RewindVisibility<'_>,
        owners: &HashMap<u64, String>,
    ) -> Result<()> {
        let Some(path) = revertable_path(event) else {
            return Ok(());
        };
        let owner = event_owner(event, owners);
        if owner != Some(conversation) {
            self.other_changes.insert(path);
        } else if visibility.includes(event, owner) {
            let path = message_rewind_path(&self.workspace, &path)?;
            self.modified.entry(path).or_insert_with(|| FirstMutation {
                id: event.id,
                created: is_created_file_write(event),
                before: None,
            });
        }
        Ok(())
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
        visibility: &RewindVisibility<'_>,
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
