use std::io;
use std::path::Path;

#[cfg(windows)]
#[path = "private_directory_windows.rs"]
mod windows;

/// Create or verify a current-user-only directory under a trusted, existing parent.
pub fn private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix_directory(path)
    }
    #[cfg(windows)]
    {
        windows::private_directory(path)
    }
    #[cfg(not(any(unix, windows)))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "private directory permissions are unavailable for {}",
            path.display()
        ),
    ))
}

#[cfg(unix)]
fn unix_directory(path: &Path) -> io::Result<()> {
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    // NOTE: geteuid reads the calling process identity. Rust has no portable equivalent.
    let owner = unsafe { libc::geteuid() };
    if metadata.is_dir() && metadata.uid() == owner && metadata.mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(insecure_directory(path))
    }
}

#[cfg(any(unix, windows))]
fn insecure_directory(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "{} must be a private directory owned by the current user",
            path.display()
        ),
    )
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use std::fs;
    use std::io::ErrorKind;

    use super::private_directory;

    #[test]
    fn creates_and_reopens_a_user_private_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private");
        private_directory(&path).unwrap();
        fs::write(path.join("content"), b"private").unwrap();
        private_directory(&path).unwrap();
        assert_eq!(
            b"private",
            fs::read(path.join("content")).unwrap().as_slice()
        );
    }

    #[test]
    fn rejects_an_existing_directory_without_private_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("open");
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            ErrorKind::PermissionDenied,
            private_directory(&path).unwrap_err().kind()
        );
    }
}
