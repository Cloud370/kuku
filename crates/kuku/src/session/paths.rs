use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

pub fn project_home(kuku_home: &Path, workspace: &Path) -> Result<PathBuf> {
    let mut path = PathBuf::from(kuku_home);
    path.push("p");

    for component in workspace.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => {}
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::InvalidWorkspacePath(
                    workspace.display().to_string(),
                ));
            }
        }
    }

    Ok(path)
}

pub fn session_events_path(kuku_home: &Path, workspace: &Path, session_id: &str) -> Result<PathBuf> {
    let mut path = project_home(kuku_home, workspace)?;
    path.push("sessions");
    path.push(session_id);
    path.push("events.jsonl");
    Ok(path)
}
