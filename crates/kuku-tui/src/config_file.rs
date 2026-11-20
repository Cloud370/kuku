//! Shared guarded publication for local configuration files.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

pub(crate) fn publish_config(
    config_path: &Path,
    original_bytes: Option<&[u8]>,
    original_permissions: Option<&fs::Permissions>,
    bytes: &[u8],
    before_compare: impl FnOnce(),
) -> io::Result<()> {
    let parent = config_path
        .parent()
        .ok_or_else(|| invalid_config("configuration path has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|_| invalid_config("cannot create configuration directory"))?;
    let canonical_path = canonical_config_path(config_path)?;
    let mut lock_path = canonical_path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let mut lock = fslock::LockFile::open(&PathBuf::from(lock_path))
        .map_err(|_| invalid_config("cannot lock configuration for update"))?;
    lock.lock()
        .map_err(|_| invalid_config("cannot lock configuration for update"))?;

    if let Some(original) = original_bytes {
        before_compare();
        let metadata = fs::symlink_metadata(config_path)
            .map_err(|_| invalid_config("configuration changed while settings were open"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(invalid_config(
                "configuration changed while settings were open",
            ));
        }
        let current = fs::read(config_path)
            .map_err(|_| invalid_config("configuration changed while settings were open"))?;
        if current != original {
            return Err(invalid_config(
                "configuration changed while settings were open; reopen settings",
            ));
        }
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|_| invalid_config("cannot stage the updated configuration"))?;
        if let Some(permissions) = original_permissions {
            temporary
                .as_file()
                .set_permissions(permissions.clone())
                .map_err(|_| invalid_config("cannot preserve configuration permissions"))?;
        }
        secure_config_temp(config_path, true, &temporary)?;
        temporary
            .write_all(bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| invalid_config("cannot write the updated configuration"))?;
        temporary
            .persist(config_path)
            .map_err(|_| invalid_config("cannot publish the updated configuration"))?;
    } else {
        before_compare();
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|_| invalid_config("cannot stage the new configuration"))?;
        secure_config_temp(config_path, false, &temporary)?;
        temporary
            .write_all(bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| invalid_config("cannot write the new configuration"))?;
        temporary
            .persist_noclobber(config_path)
            .map_err(|_| invalid_config("configuration already exists; it was not overwritten"))?;
    }
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn canonical_config_path(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| invalid_config("configuration path has no parent directory"))?;
            let name = path
                .file_name()
                .ok_or_else(|| invalid_config("configuration path has no file name"))?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn secure_config_temp(
    config_path: &Path,
    existing: bool,
    temporary: &NamedTempFile,
) -> io::Result<()> {
    kuku::util::set_config_temp_security(
        existing.then_some(config_path),
        temporary.path(),
        temporary.as_file(),
    )
    .map_err(|_| invalid_config("cannot secure configuration file before writing"))
}

#[cfg(not(windows))]
fn secure_config_temp(
    _config_path: &Path,
    _existing: bool,
    _temporary: &NamedTempFile,
) -> io::Result<()> {
    Ok(())
}

fn invalid_config(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
