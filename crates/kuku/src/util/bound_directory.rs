use std::fs::File;
use std::io;
use std::path::{Component, Path};

#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd};

#[cfg(unix)]
pub(crate) struct BoundDirectory(File);

#[cfg(windows)]
pub(crate) struct BoundDirectory {
    _components: Vec<File>,
}

impl BoundDirectory {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Self::walk(path, None)
    }

    pub(crate) fn open_or_create(
        path: &Path,
        before_create: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::walk(path, Some(before_create))
    }

    #[cfg(unix)]
    fn walk(path: &Path, before_create: Option<&dyn Fn() -> io::Result<()>>) -> io::Result<Self> {
        use rustix::fs::{Mode, mkdirat, open, openat};
        use rustix::io::Errno;

        let flags = directory_flags();
        let mut components = path.components();
        if components.next() != Some(Component::RootDir) {
            return Err(invalid_directory());
        }
        let mut current = open("/", flags, Mode::empty())?;
        for component in components {
            let Component::Normal(name) = component else {
                return Err(invalid_directory());
            };
            current = match openat(&current, name, flags, Mode::empty()) {
                Ok(next) => next,
                Err(Errno::NOENT) if let Some(check) = before_create => {
                    check()?;
                    match mkdirat(&current, name, Mode::from_bits_truncate(0o777)) {
                        Ok(()) | Err(Errno::EXIST) => {}
                        Err(error) => return Err(error.into()),
                    }
                    let next = openat(&current, name, flags, Mode::empty())?;
                    sync_directory(&current)?;
                    next
                }
                Err(error) => return Err(error.into()),
            };
        }
        Ok(Self(File::from(current)))
    }

    #[cfg(unix)]
    pub(crate) fn sync(&self) -> io::Result<()> {
        sync_directory(self)
    }

    #[cfg(windows)]
    fn walk(path: &Path, before_create: Option<&dyn Fn() -> io::Result<()>>) -> io::Result<Self> {
        use std::fs::{self, OpenOptions};
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use std::path::PathBuf;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
        };

        if !path.is_absolute() {
            return Err(invalid_directory());
        }
        let options = OpenOptions::new()
            .access_mode(FILE_TRAVERSE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .clone();
        let mut dir = PathBuf::new();
        let mut components = Vec::new();
        for component in path.components() {
            dir.push(component.as_os_str());
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            // Holding every component without delete sharing keeps the path bound through use.
            let file = match options.open(&dir) {
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && let Some(check) = before_create =>
                {
                    check()?;
                    match fs::create_dir(&dir) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    options.open(&dir)?
                }
                result => result?,
            };
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

#[cfg(unix)]
impl AsFd for BoundDirectory {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

#[cfg(unix)]
fn directory_flags() -> rustix::fs::OFlags {
    use rustix::fs::OFlags;

    #[cfg(target_os = "linux")]
    let access = OFlags::PATH;
    #[cfg(target_os = "macos")]
    let access = OFlags::from_bits_retain(libc::O_SEARCH as _);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let access = OFlags::RDONLY;
    access | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW
}

#[cfg(unix)]
fn sync_directory(directory: &impl AsFd) -> io::Result<()> {
    use rustix::fs::{Mode, OFlags, fsync, openat};

    let readable = openat(
        directory,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    fsync(readable)?;
    Ok(())
}

fn invalid_directory() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "expected a canonical directory",
    )
}
