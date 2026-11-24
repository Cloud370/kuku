use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use kuku::util::sync_private_file;
use tempfile::Builder;

use super::{Settings, invalid_config};

pub(super) struct StagedKey {
    path: PathBuf,
    pub(super) keep: bool,
}

impl StagedKey {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedKey {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) fn stage_key(settings: &Settings, key: &str) -> io::Result<StagedKey> {
    stage_key_with_sync(settings, key, |file_path, directory_created| {
        sync_private_file(file_path)?;
        #[cfg(unix)]
        if directory_created {
            let directory = file_path
                .parent()
                .ok_or_else(|| invalid_config("credential file has no parent directory"))?;
            sync_private_file(directory)?;
        }
        #[cfg(not(unix))]
        let _ = directory_created;
        Ok(())
    })
}

pub(super) fn stage_key_with_sync(
    settings: &Settings,
    key: &str,
    sync_publication: impl FnOnce(&Path, bool) -> io::Result<()>,
) -> io::Result<StagedKey> {
    let directory_created = match fs::symlink_metadata(&settings.credentials_dir) {
        Ok(_) => false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(_) => {
            return Err(invalid_config(
                "cannot create a private credential directory",
            ));
        }
    };
    settings.check_storage(true).map_err(|_| {
        invalid_config("stored keys are unavailable; use Environment or a different --data-dir")
    })?;
    let mut file = Builder::new()
        .prefix("key-")
        .suffix(".credential")
        .tempfile_in(&settings.credentials_dir)
        .map_err(|_| invalid_config("cannot create a private credential file"))?;
    #[cfg(windows)]
    kuku::util::set_config_temp_security(None, file.path(), file.as_file())
        .map_err(|_| invalid_config("cannot secure the private credential file"))?;
    file.write_all(key.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| invalid_config("cannot save the private credential file"))?;
    let (_, path) = file
        .keep()
        .map_err(|_| invalid_config("cannot save the private credential file"))?;
    let staged = StagedKey { path, keep: false };
    sync_publication(&staged.path, directory_created)
        .map_err(|_| invalid_config("cannot sync private credential storage"))?;
    Ok(staged)
}
