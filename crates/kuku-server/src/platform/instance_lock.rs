use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub struct ServerInstanceLock {
    lock: fslock::LockFile,
    path: PathBuf,
}

#[derive(Debug)]
pub enum InstanceLockError {
    AlreadyRunning,
    Io(io::Error),
}

impl fmt::Display for InstanceLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => formatter.write_str("another web server is already running"),
            Self::Io(error) => write!(formatter, "cannot acquire web server lock: {error}"),
        }
    }
}

impl std::error::Error for InstanceLockError {}

impl ServerInstanceLock {
    pub fn acquire(kuku_home: &Path) -> Result<Self, InstanceLockError> {
        std::fs::create_dir_all(kuku_home).map_err(InstanceLockError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(kuku_home, std::fs::Permissions::from_mode(0o700))
                .map_err(InstanceLockError::Io)?;
        }
        let path = kuku_home.join("web.lock");
        let mut lock = fslock::LockFile::open(&path)
            .map_err(|error| InstanceLockError::Io(io::Error::other(error)))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(InstanceLockError::Io)?;
        }
        match lock.try_lock() {
            Ok(true) => Ok(Self { lock, path }),
            Ok(false) => Err(InstanceLockError::AlreadyRunning),
            Err(error) => Err(InstanceLockError::Io(io::Error::other(error))),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ServerInstanceLock {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}
