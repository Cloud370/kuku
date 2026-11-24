use std::fs::File;
use std::io;
use std::path::{Component, Path};

#[cfg(unix)]
pub(super) struct CommandDirectory(File);

#[cfg(unix)]
impl CommandDirectory {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;

        if !path.is_absolute() {
            return Err(invalid_directory());
        }
        #[cfg(target_os = "linux")]
        let access = libc::O_PATH;
        #[cfg(not(target_os = "linux"))]
        let access = libc::O_RDONLY;
        let flags = access | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        // SAFETY: The path is a static C string. The successful call returns an owned descriptor.
        let fd = unsafe { libc::open(c"/".as_ptr(), flags) };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: The successful open transferred ownership of this descriptor to us.
        let mut directory = unsafe { File::from_raw_fd(fd) };
        for component in path.components().skip(1) {
            let Component::Normal(name) = component else {
                return Err(invalid_directory());
            };
            let name = CString::new(name.as_bytes()).map_err(|_| invalid_directory())?;
            // SAFETY: The borrowed directory and C string remain live for the call.
            let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            if fd == -1 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: The successful openat returned a new descriptor owned by this process.
            directory = unsafe { File::from_raw_fd(fd) };
        }
        Ok(Self(directory))
    }

    pub(super) fn configure(&self, command: &mut tokio::process::Command) -> io::Result<()> {
        use std::os::fd::AsRawFd;

        let directory = self.0.try_clone()?;
        // SAFETY: The closure holds an open directory and only calls async-signal-safe fchdir.
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(directory.as_raw_fd()) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

#[cfg(windows)]
pub(super) struct CommandDirectory {
    _components: Vec<File>,
}

#[cfg(windows)]
impl CommandDirectory {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        use std::fs::OpenOptions;
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use std::path::PathBuf;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_LIST_DIRECTORY, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        if !path.is_absolute() {
            return Err(invalid_directory());
        }
        let mut dir = PathBuf::new();
        let mut components = Vec::new();
        for component in path.components() {
            dir.push(component.as_os_str());
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            // Hold each directory through process creation to prevent rename or deletion.
            let file = OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&dir)?;
            let metadata = file.metadata()?;
            if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(invalid_directory());
            }
            components.push(file);
        }
        Ok(Self {
            _components: components,
        })
    }
}

fn invalid_directory() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid prepared command directory",
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::CommandDirectory;

    #[tokio::test]
    async fn process_uses_the_open_directory_if_the_path_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let original = root.join("approved");
        let moved = root.join("moved");
        let replacement = root.join("replacement");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        let directory = CommandDirectory::open(&original).unwrap();

        std::fs::rename(&original, &moved).unwrap();
        symlink(&replacement, &original).unwrap();
        assert!(CommandDirectory::open(&original).is_err());

        let mut command = tokio::process::Command::new("pwd");
        directory.configure(&mut command).unwrap();
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(
            moved.to_str().unwrap(),
            std::str::from_utf8(&output.stdout).unwrap().trim()
        );
    }
}
