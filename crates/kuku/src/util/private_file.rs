use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const MAX_KEY_BYTES: u64 = 4096;

pub(crate) fn read_private_key(path: &Path) -> io::Result<String> {
    let file = open_private_key(path)?;
    if file.metadata()?.len() > MAX_KEY_BYTES {
        return Err(invalid_contents());
    }
    let mut bytes = Vec::new();
    file.take(MAX_KEY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid_contents())?;
    if bytes.len() as u64 > MAX_KEY_BYTES {
        return Err(invalid_contents());
    }
    let key = String::from_utf8(bytes).map_err(|_| invalid_contents())?;
    if key.trim().is_empty() || key.chars().any(char::is_control) {
        return Err(invalid_contents());
    }
    Ok(key)
}

/// Apply the platform's durable publication sync policy to a private file path.
///
/// Unix syncs the containing directory. Windows reopens the file with write-through and syncs it.
pub fn sync_private_file(path: &Path) -> io::Result<()> {
    crate::util::fs_sync::sync_parent(path)
}

#[doc(hidden)]
pub fn is_valid_private_key(path: &Path) -> bool {
    read_private_key(path).is_ok()
}

fn invalid_contents() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "stored credential file must contain one non-empty UTF-8 line",
    )
}

#[cfg(unix)]
fn open_private_key(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| unsafe_file())?;
    let metadata = file.metadata()?;
    // NOTE: geteuid reads the caller identity without pointers; std exposes no safe effective-UID query.
    let owner = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(unsafe_file());
    }
    Ok(file)
}

#[cfg(windows)]
fn open_private_key(path: &Path) -> io::Result<File> {
    super::private_directory::open_private_key(path)
}

#[cfg(not(any(unix, windows)))]
fn open_private_key(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private credential files are unsupported on this platform",
    ))
}

#[cfg(unix)]
fn unsafe_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "stored credential file is not a private regular file",
    )
}

#[cfg(test)]
#[path = "private_file_tests.rs"]
mod tests;
