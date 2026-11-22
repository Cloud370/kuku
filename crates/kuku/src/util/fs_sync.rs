use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
mod directory;

pub(crate) fn write_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        options.custom_flags(FILE_FLAG_WRITE_THROUGH);
    }
    options
}

pub(crate) fn lock_writer(file: &File, path: &Path) -> io::Result<()> {
    file.try_lock().map_err(|error| lock_error(path, error))
}

pub(crate) fn lock_reader(file: &File, path: &Path) -> io::Result<()> {
    file.try_lock_shared()
        .map_err(|error| lock_error(path, error))
}

pub(crate) fn lock_journal(file: &File, path: &Path, exclusive: bool) -> io::Result<()> {
    #[cfg(windows)]
    return windows::lock_journal(file, exclusive).map_err(|error| lock_error(path, error));
    #[cfg(not(windows))]
    if exclusive {
        lock_writer(file, path)
    } else {
        lock_reader(file, path)
    }
}

pub(crate) fn unlock_journal(file: &File) -> io::Result<()> {
    #[cfg(windows)]
    return windows::unlock_journal(file);
    #[cfg(not(windows))]
    file.unlock()
}

pub(crate) fn create_dir_all(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        directory::create_dir_all(path)
    }
    #[cfg(not(unix))]
    {
        // NOTE: Windows publication uses write-through file handles and flushes instead of directory fsync.
        fs::create_dir_all(path)
    }
}

pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    #[cfg(windows)]
    {
        file.sync_all()
    }
    #[cfg(not(windows))]
    {
        file.sync_data()
    }
}

pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let absolute = std::env::current_dir()?.join(path);
        sync_directory(absolute.parent().unwrap())
    }
    #[cfg(windows)]
    {
        write_options().open(path)?.sync_all()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "durable file publication is not supported on this platform",
        ))
    }
}

pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::rename(from, to)
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)?;
        sync_parent(to)
    }
}

pub(crate) fn replace_journal(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    return windows::replace_journal(from, to);
    #[cfg(not(windows))]
    rename(from, to)
}

fn lock_error(path: &Path, error: fs::TryLockError) -> io::Error {
    match error {
        fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "event store {} already has an active writer or reader",
                path.display()
            ),
        ),
        fs::TryLockError::Error(error) => io::Error::new(
            error.kind(),
            format!("cannot lock event file {}: {error}", path.display()),
        ),
    }
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    let directory = File::open(path)?;
    if !directory.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("cannot sync directory {}: not a directory", path.display()),
        ));
    }
    directory.sync_all().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot sync directory {}: {error}", path.display()),
        )
    })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn sync_directory(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory sync is not supported on this platform",
    ))
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GENERIC_WRITE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfoEx, LockFileEx, SetFileInformationByHandle, UnlockFileEx, DELETE,
        FILE_FLAG_WRITE_THROUGH, FILE_RENAME_INFO, LOCKFILE_EXCLUSIVE_LOCK,
        LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }

    fn journal_lock_offset() -> OVERLAPPED {
        // NOTE: Windows locks are mandatory. Fence journal handles beyond the data range.
        OVERLAPPED {
            Anonymous: OVERLAPPED_0 {
                Anonymous: OVERLAPPED_0_0 {
                    Offset: u32::MAX - 1,
                    OffsetHigh: i32::MAX as u32,
                },
            },
            ..OVERLAPPED::default()
        }
    }

    pub(super) fn lock_journal(file: &fs::File, exclusive: bool) -> Result<(), fs::TryLockError> {
        let mut position = journal_lock_offset();
        let flags = LOCKFILE_FAIL_IMMEDIATELY
            | if exclusive {
                LOCKFILE_EXCLUSIVE_LOCK
            } else {
                0
            };
        // SAFETY: The File and synchronous OVERLAPPED storage remain live during the call.
        if unsafe { LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &mut position) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        Err(
            if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
                fs::TryLockError::WouldBlock
            } else {
                fs::TryLockError::Error(error)
            },
        )
    }

    pub(super) fn unlock_journal(file: &fs::File) -> io::Result<()> {
        let mut position = journal_lock_offset();
        // SAFETY: The same handle and byte range used during locking are live for this call.
        if unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut position) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn rename(from: &Path, to: &Path) -> io::Result<()> {
        let from = absolute_name(from)?;
        let to = absolute_name(to)?;
        // NOTE: Both buffers are live, NUL-terminated UTF-16 paths; std::fs::rename cannot set WRITE_THROUGH.
        let moved = unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn replace_journal(from: &Path, to: &Path) -> io::Result<()> {
        match rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.raw_os_error().map(|code| code as u32),
                    Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
                ) => {}
            Err(error) => return Err(error),
        }
        const REPLACE_IF_EXISTS: u32 = 1;
        const POSIX_SEMANTICS: u32 = 2;
        let target = absolute_name(to)?;
        let name_bytes = std::mem::size_of_val(target.as_slice());
        let size = (std::mem::offset_of!(FILE_RENAME_INFO, FileName) + name_bytes)
            .max(std::mem::size_of::<FILE_RENAME_INFO>());
        let size_u32 = u32::try_from(size)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "journal path is too long"))?;
        let mut data = vec![0_usize; size.div_ceil(std::mem::size_of::<usize>())];
        let info = data.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        let file = fs::OpenOptions::new()
            .access_mode(DELETE | GENERIC_WRITE)
            .custom_flags(FILE_FLAG_WRITE_THROUGH)
            .open(from)?;
        // SAFETY: The aligned buffer covers the header and NUL-terminated target name.
        let renamed = unsafe {
            (*info).Anonymous.Flags = REPLACE_IF_EXISTS | POSIX_SEMANTICS;
            (*info).FileNameLength = (name_bytes - std::mem::size_of::<u16>()) as u32;
            ptr::copy_nonoverlapping(
                target.as_ptr(),
                ptr::addr_of_mut!((*info).FileName).cast(),
                target.len(),
            );
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileRenameInfoEx,
                info.cast(),
                size_u32,
            )
        };
        if renamed == 0 {
            return Err(io::Error::last_os_error());
        }
        file.sync_all()
    }

    fn absolute_name(path: &Path) -> io::Result<Vec<u16>> {
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "file path has no final component",
            )
        })?;
        let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
        let absolute = fs::canonicalize(parent.unwrap_or_else(|| Path::new(".")))?.join(name);
        wide_path(absolute.as_os_str())
    }

    fn wide_path(path: &OsStr) -> io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = path.encode_wide().collect();
        if wide.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file path contains a NUL character",
            ));
        }
        wide.push(0);
        Ok(wide)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        create_dir_all, lock_reader, lock_writer, rename, sync_file, sync_parent, write_options,
    };

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> PathBuf {
        let root = std::env::var_os("KUKU_FS_SYNC_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        root.join(format!(
            "kuku-fs-sync-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed),
        ))
    }

    #[test]
    fn creates_appends_reopens_and_publishes_files() {
        let root = test_directory();
        let nested = root.join("session").join("outputs");
        create_dir_all(&nested).unwrap();
        let temporary = nested.join("pending");
        let final_path = nested.join("output");
        let mut file = write_options().create_new(true).open(&temporary).unwrap();
        file.write_all(b"first").unwrap();
        sync_file(&file).unwrap();
        sync_parent(&temporary).unwrap();
        drop(file);
        let mut file = write_options().append(true).open(&temporary).unwrap();
        file.write_all(b" second").unwrap();
        sync_file(&file).unwrap();
        drop(file);
        rename(&temporary, &final_path).unwrap();
        assert_eq!(b"first second", fs::read(&final_path).unwrap().as_slice());
        assert!(!temporary.exists());
        let mut file = write_options().create_new(true).open(&temporary).unwrap();
        file.write_all(b"replacement").unwrap();
        sync_file(&file).unwrap();
        drop(file);
        rename(&temporary, &final_path).unwrap();
        assert_eq!(b"replacement", fs::read(&final_path).unwrap().as_slice());
        create_dir_all(&nested).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_errors_preserve_the_existing_file() {
        let root = test_directory();
        create_dir_all(&root).unwrap();
        let path = root.join("existing");
        let mut file = write_options().create_new(true).open(&path).unwrap();
        file.write_all(b"original").unwrap();
        sync_file(&file).unwrap();
        drop(file);
        #[cfg(windows)]
        assert!(sync_file(&File::open(&path).unwrap()).is_err());
        assert!(rename(&root.join("missing"), &path).is_err());
        assert!(rename(&path, &root.join("missing").join("target")).is_err());
        assert!(create_dir_all(&path.join("child")).is_err());
        assert!(sync_parent(&path.join("child")).is_err());
        assert!(write_options().create_new(true).open(&path).is_err());
        assert_eq!(b"original", fs::read(&path).unwrap().as_slice());
        assert!(File::open(root.join("missing")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_locks_follow_hardlink_aliases_and_release_with_the_handle() {
        if let Some(path) = std::env::var_os("KUKU_FS_SYNC_LOCK_CHILD") {
            let path = PathBuf::from(path);
            let writer = write_options().read(true).append(true).open(&path).unwrap();
            assert_eq!(
                std::io::ErrorKind::WouldBlock,
                lock_writer(&writer, &path).unwrap_err().kind()
            );
            let reader = File::open(&path).unwrap();
            assert_eq!(
                std::io::ErrorKind::WouldBlock,
                lock_reader(&reader, &path).unwrap_err().kind()
            );
            return;
        }
        let root = test_directory();
        create_dir_all(&root).unwrap();
        let path = root.join("events");
        let alias = root.join("alias");
        let mut writer = write_options()
            .read(true)
            .append(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        lock_writer(&writer, &path).unwrap();
        writer.write_all(b"record\n").unwrap();
        sync_file(&writer).unwrap();
        sync_parent(&path).unwrap();
        fs::hard_link(&path, &alias).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env("KUKU_FS_SYNC_LOCK_CHILD", &alias)
            .arg("--exact")
            .arg(std::thread::current().name().unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        drop(writer);

        let reader = File::open(&alias).unwrap();
        lock_reader(&reader, &alias).unwrap();
        let other_reader = File::open(&path).unwrap();
        lock_reader(&other_reader, &path).unwrap();
        let writer = write_options().read(true).append(true).open(&path).unwrap();
        assert_eq!(
            std::io::ErrorKind::WouldBlock,
            lock_writer(&writer, &path).unwrap_err().kind()
        );
        drop(reader);
        assert!(lock_writer(&writer, &path).is_err());
        drop(other_reader);
        lock_writer(&writer, &path).unwrap();
        drop(writer);
        fs::remove_dir_all(root).unwrap();
    }
}
