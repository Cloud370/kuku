use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

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

pub(crate) fn create_dir_all(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    // NOTE: Windows publication uses write-through file handles and flushes instead of directory fsync.
    #[cfg(unix)]
    for directory in fs::canonicalize(path)?.ancestors() {
        sync_directory(directory)?;
    }
    Ok(())
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

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
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

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
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

    use super::{create_dir_all, rename, sync_file, sync_parent, write_options};

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
}
