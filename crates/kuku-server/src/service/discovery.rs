use std::ffi::OsString;
use std::fs::{self, TryLockError};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tempfile::NamedTempFile;

use super::access::new_token;

const DISCOVERY_NAME: &str = "connection.json";

#[derive(Serialize)]
pub(super) struct Discovery {
    pub(super) address: SocketAddr,
    pub(super) token: String,
    pub(super) instance_id: String,
    pub(super) pid: u32,
    config_path: OsString,
    data_dir: OsString,
}

impl Discovery {
    pub(super) fn new(
        address: SocketAddr,
        data_dir: &Path,
        config_path: &Path,
    ) -> io::Result<Self> {
        Ok(Self {
            address,
            token: new_token()?,
            instance_id: new_token()?,
            pid: std::process::id(),
            config_path: config_path.as_os_str().to_owned(),
            data_dir: data_dir.as_os_str().to_owned(),
        })
    }
}

pub(super) struct Lease {
    directory: PathBuf,
    _lock: fs::File,
}

impl Lease {
    pub(super) fn acquire(data_dir: &Path) -> io::Result<Self> {
        let directory = data_dir.join("service");
        kuku::util::private_directory(&directory)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("instance.lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "a managed server already owns {}; run kuku server status",
                        data_dir.display()
                    ),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        remove_discovery(&directory)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    pub(super) fn publish(&self, discovery: &Discovery) -> io::Result<()> {
        let mut file = NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&mut file, discovery)?;
        file.flush()?;
        file.persist(self.directory.join(DISCOVERY_NAME))?;
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Err(error) = remove_discovery(&self.directory) {
            tracing::warn!(%error, "could not remove managed server discovery");
        }
    }
}

fn remove_discovery(directory: &Path) -> io::Result<()> {
    let path = directory.join(DISCOVERY_NAME);
    #[cfg(windows)]
    let removed = {
        // NOTE: Unpublish the name while existing readers finish. DeleteFile can leave
        // the name pending deletion and deny new opens until the last reader closes.
        let retired = NamedTempFile::new_in(directory)?.into_temp_path();
        fs::rename(path, &retired).and_then(|()| retired.close())
    };
    #[cfg(not(windows))]
    let removed = fs::remove_file(path);
    match removed {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Read};

    use super::{remove_discovery, DISCOVERY_NAME};

    #[test]
    fn withdrawal_hides_discovery_while_an_existing_reader_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(DISCOVERY_NAME);
        fs::write(&path, b"ready").unwrap();
        let mut reader = fs::File::open(&path).unwrap();

        remove_discovery(directory.path()).unwrap();
        assert_eq!(
            io::ErrorKind::NotFound,
            fs::File::open(&path).unwrap_err().kind()
        );
        fs::write(&path, b"replacement").unwrap();

        let mut original = String::new();
        reader.read_to_string(&mut original).unwrap();
        assert_eq!("ready", original);
        drop(reader);
        assert_eq!("replacement", fs::read_to_string(&path).unwrap());
    }
}
