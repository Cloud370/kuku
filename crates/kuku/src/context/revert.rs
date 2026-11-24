//! File restore and journal visibility for Session rewinds.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::event::history::{
    event_conversation as conversation_event_conversation, event_turn as conversation_event_turn,
};
use crate::event::{EventPayload, StoredEvent};

#[path = "revert_apply.rs"]
mod apply;
#[path = "revert_message_rewind.rs"]
mod message_rewind;

#[cfg(test)]
pub(crate) use apply::apply_message_rewind_files;
pub(crate) use apply::{FileRevertTransaction, apply_verified_file_rewind};
pub(crate) use message_rewind::event_owner_with;
pub use message_rewind::event_owners;
use message_rewind::{event_owner, event_turn_owners};
#[path = "revert_message_plan.rs"]
mod message_plan;
pub use message_plan::compute_message_rewind_plan;
pub(crate) use message_plan::plan_file_rewind;
pub use message_plan::{FileRewindChange, FileRewindPreview};

const REVERTABLE_KINDS: &[&str] = &["file_edit", "file_write", "memory_write", "forget_memory"];

/// Plan for reverting files to their state before a target turn.
pub struct RevertPlan {
    pub restores: Vec<FileRestore>,
    pub deletes: Vec<PathBuf>,
    pub unrecoverable: Vec<PathBuf>,
    pub sensitive_files: Vec<PathBuf>,
}

/// A single file to be restored to its previous content.
pub struct FileRestore {
    pub path: PathBuf,
    pub old_content: String,
    pub old_hash: String,
    pub new_content_on_plan: String,
}

/// Filter events excluded by the current Session rewind history.
pub fn filter_rolled_back_events(events: &[StoredEvent]) -> Vec<&StoredEvent> {
    let visibility = RewindVisibility::new(events);
    let turn_owners = event_turn_owners(events);
    events
        .iter()
        .filter(|event| visibility.includes(event, event_owner(event, &turn_owners)))
        .collect()
}

pub(crate) struct RewindVisibility {
    hidden: HashMap<String, Vec<std::ops::Range<u64>>>,
}

impl RewindVisibility {
    pub(crate) fn new(controls: &[StoredEvent]) -> Self {
        use crate::event::history::{hidden_history, history_cut};
        let addresses: HashSet<_> = controls
            .iter()
            .filter_map(|event| history_cut(event).map(|(agent, _)| agent))
            .collect();
        Self {
            hidden: addresses
                .into_iter()
                .map(|agent| (agent.to_owned(), hidden_history(controls.iter(), agent)))
                .collect(),
        }
    }

    pub(crate) fn hidden_start(&self, agent: &str, id: u64) -> Option<u64> {
        crate::event::history::hidden_range(self.hidden.get(agent)?, id).map(|range| range.start)
    }

    pub(crate) fn includes(&self, event: &StoredEvent, owner: Option<&str>) -> bool {
        owner
            .and_then(|owner| self.hidden_start(owner, event.id))
            .is_none()
    }
}

fn revertable_path(event: &StoredEvent) -> Option<String> {
    let EventPayload::ToolResult {
        structured: Some(value),
        ..
    } = &event.payload
    else {
        return None;
    };
    REVERTABLE_KINDS
        .contains(&value["kind"].as_str()?)
        .then(|| {
            value["canonical_path"]
                .as_str()
                .or_else(|| value["path"].as_str())
                .map(str::to_owned)
        })?
}

fn message_rewind_path(workspace: &Path, path: &str) -> crate::Result<PathBuf> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Err(crate::Error::InvalidArgument(format!(
            "cannot safely restore ambiguous relative file path without its Agent workspace: {path}"
        )));
    }
    let candidate = candidate.to_path_buf();
    reject_symlink_components(workspace, &candidate)?;
    resolve_revert_path(workspace, path).ok_or_else(|| {
        crate::Error::InvalidArgument(format!("file path is outside the workspace: {path}"))
    })
}

fn message_rewind_comparison_path(path: &str) -> crate::Result<PathBuf> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Err(crate::Error::InvalidArgument(format!(
            "cannot safely compare ambiguous relative file path without its Agent workspace: {path}"
        )));
    }
    canonicalize_event_path(candidate).map_err(crate::Error::Io)
}

fn reject_symlink_components(workspace: &Path, path: &Path) -> crate::Result<()> {
    let relative = path.strip_prefix(workspace).map_err(|_| {
        crate::Error::InvalidArgument(format!(
            "file path is outside the workspace: {}",
            path.display()
        ))
    })?;
    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(name) => current.push(name),
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                if !current.starts_with(workspace) {
                    return Err(crate::Error::InvalidArgument(format!(
                        "file path is outside the workspace: {}",
                        path.display()
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(crate::Error::InvalidArgument(format!(
                    "file path is outside the workspace: {}",
                    path.display()
                )));
            }
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(crate::Error::InvalidArgument(format!(
                    "cannot restore through a symbolic link: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(crate::Error::Io(error)),
        }
    }
    Ok(())
}

fn resolve_revert_path(workspace: &Path, event_path: &str) -> Option<PathBuf> {
    let candidate = Path::new(event_path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    canonicalize_revert_target(workspace, &joined).ok()
}

fn canonicalize_revert_target(workspace: &Path, path: &Path) -> Result<PathBuf, std::io::Error> {
    let resolved = canonicalize_event_path(path)?;
    if resolved.starts_with(workspace) {
        Ok(resolved)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("path outside workspace: {}", resolved.display()),
        ))
    }
}

fn canonicalize_event_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    let normalized =
        crate::tool::builtin::common::normalize_existing_components(path.to_path_buf());
    let mut existing = normalized.as_path();
    let mut suffix = Vec::<OsString>::new();

    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("path outside workspace: {}", normalized.display()),
            ));
        };
        suffix.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("path outside workspace: {}", normalized.display()),
            )
        })?;
    }

    let mut resolved = existing.canonicalize()?;
    while let Some(component) = suffix.pop() {
        resolved.push(component);
    }

    Ok(resolved)
}

fn ensure_path_within_workspace(workspace: &Path, path: &Path) -> Result<(), std::io::Error> {
    canonicalize_revert_target(workspace, path).map(|_| ())
}

#[cfg(test)]
#[path = "revert_tests.rs"]
mod tests;
