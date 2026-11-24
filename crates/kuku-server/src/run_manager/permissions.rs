use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use kuku::permission::PermissionMode;
use kuku::session::project_home;

use super::{RunManager, Slot};
use crate::error_mapping::Result;

impl RunManager {
    pub(crate) fn workspace_permission(
        &self,
        home: &Path,
        workspace: &Path,
    ) -> Result<Option<PermissionMode>> {
        let mut modes = self.inner.workspace_permissions.lock().unwrap();
        if let Some(mode) = modes.get(workspace) {
            return Ok(*mode);
        }
        let mode = match fs::read(permission_path(home, workspace)?) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(kuku::Error::from)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        modes.insert(workspace.to_owned(), mode);
        Ok(mode)
    }

    pub(crate) fn cached_workspace_permission(&self, workspace: &Path) -> Option<PermissionMode> {
        self.inner
            .workspace_permissions
            .lock()
            .unwrap()
            .get(workspace)
            .copied()
            .flatten()
    }

    pub(crate) fn set_workspace_permission(
        &self,
        home: &Path,
        workspace: &Path,
        mode: PermissionMode,
    ) -> Result<()> {
        let mut modes = self.inner.workspace_permissions.lock().unwrap();
        let path = permission_path(home, workspace)?;
        let dir = path.parent().expect("workspace preference has a parent");
        fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut file, &mode).map_err(kuku::Error::from)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        modes.insert(workspace.to_owned(), Some(mode));

        let slots = self.inner.slots.lock().unwrap();
        for slot in slots.values().filter(|slot| slot.workspace == workspace) {
            if let Some(session) = slot.session.lock().unwrap().as_ref() {
                session.set_permission_mode(mode);
            }
        }
        self.inner.changed.send_replace(());
        Ok(())
    }

    pub(super) fn publish_session_permissions(&self, slot: &Slot, session: &kuku::Session) {
        let modes = self.inner.workspace_permissions.lock().unwrap();
        if let Some(Some(mode)) = modes.get(&slot.workspace) {
            session.set_permission_mode(*mode);
        }
        *slot.session.lock().unwrap() = Some(session.clone());
    }
}

fn permission_path(home: &Path, workspace: &Path) -> Result<PathBuf> {
    Ok(project_home(home, workspace)?.join("permission-mode.json"))
}
