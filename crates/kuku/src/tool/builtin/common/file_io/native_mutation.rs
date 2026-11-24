use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io::{self, Write};

use rustix::fs::{AtFlags, FileType, Mode, OFlags, linkat, openat, renameat, statat, unlinkat};
use rustix::io::Errno;

use crate::util::bound_directory::BoundDirectory;

use super::{
    FILE_BUFFER_BYTES, FileCancellation, Publication, StableFile, WorkspaceCommitError,
    changed_during_read, confirm_publication, ensure_active, same_version_marker,
    scan_open_file_version, version_error,
};

pub(super) fn publish_file(
    directory: &BoundDirectory,
    name: &OsStr,
    expected: Option<&str>,
    bytes: &[u8],
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> Result<Publication, WorkspaceCommitError> {
    let (mut staged, mut file) = StagedFile::create(directory)?;
    for chunk in bytes.chunks(FILE_BUFFER_BYTES) {
        ensure_active(cancelled)?;
        file.write_all(chunk)?;
    }
    if expected.is_some() {
        let existing = open_regular(directory, name).map_err(version_error)?;
        file.set_permissions(existing.metadata()?.permissions())?;
    }
    file.flush()?;
    file.sync_data()?;
    if let Some(expected) = expected {
        verify_version(directory, name, expected, scan_bytes, cancelled)?;
        ensure_active(cancelled)?;
        renameat(directory, staged.name.as_str(), directory, name).map_err(io::Error::from)?;
    } else {
        ensure_active(cancelled)?;
        return staged.persist_noclobber(name).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                WorkspaceCommitError::Conflict
            } else {
                WorkspaceCommitError::WriteFailed(error)
            }
        });
    }
    staged.published = true;
    Ok(confirm_publication(directory.sync()))
}

pub(super) fn remove_file(
    directory: &BoundDirectory,
    name: &OsStr,
    expected: &str,
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> Result<(Publication, String, u64), WorkspaceCommitError> {
    let (version, metadata) = verify_version(directory, name, expected, scan_bytes, cancelled)?;
    ensure_active(cancelled)?;
    unlinkat(directory, name, AtFlags::empty()).map_err(|error| version_error(error.into()))?;
    Ok((
        confirm_publication(directory.sync()),
        version,
        metadata.len(),
    ))
}

fn verify_version(
    directory: &BoundDirectory,
    name: &OsStr,
    expected: &str,
    scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> Result<(String, Metadata), WorkspaceCommitError> {
    ensure_active(cancelled)?;
    let file = open_regular(directory, name).map_err(version_error)?;
    let opened = file.metadata()?;
    let (version, metadata, _) = scan_open_file_version(
        StableFile {
            file,
            opened,
            cancelled,
        },
        scan_bytes,
        None,
        |file| {
            ensure_active(file.cancelled)?;
            let named = open_regular(directory, name)?.metadata()?;
            if !same_version_marker(&file.opened, &named)
                || !same_version_marker(&file.opened, &file.file.metadata()?)
            {
                return Err(changed_during_read());
            }
            Ok(())
        },
    )
    .map_err(version_error)?;
    if !super::super::snapshot::version_matches(&version, expected) {
        return Err(WorkspaceCommitError::Conflict);
    }
    Ok((version, metadata))
}

fn open_regular(directory: &BoundDirectory, name: &OsStr) -> io::Result<File> {
    let stat = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(changed_during_read());
    }
    let file = File::from(openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(changed_during_read());
    }
    Ok(file)
}

struct StagedFile<'a> {
    directory: &'a BoundDirectory,
    name: String,
    published: bool,
}

impl<'a> StagedFile<'a> {
    fn create(directory: &'a BoundDirectory) -> io::Result<(Self, File)> {
        let name = format!(".kuku-{}", uuid::Uuid::new_v4().simple());
        let file = File::from(openat(
            directory,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        Ok((
            Self {
                directory,
                name,
                published: false,
            },
            file,
        ))
    }
    fn persist_noclobber(mut self, name: &OsStr) -> io::Result<Publication> {
        let directory = self.directory;
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        {
            use rustix::fs::{RenameFlags, renameat_with};

            match renameat_with(
                directory,
                self.name.as_str(),
                directory,
                name,
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => {
                    self.published = true;
                    return Ok(confirm_publication(directory.sync()));
                }
                Err(Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP) => {}
                Err(error) => return Err(error.into()),
            }
        }
        linkat(
            directory,
            self.name.as_str(),
            directory,
            name,
            AtFlags::empty(),
        )?;
        if let Err(error) = unlinkat(directory, self.name.as_str(), AtFlags::empty()) {
            return Ok(Publication::Unconfirmed(error.into()));
        }
        self.published = true;
        Ok(confirm_publication(directory.sync()))
    }
}

impl Drop for StagedFile<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = unlinkat(self.directory, self.name.as_str(), AtFlags::empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use super::{BoundDirectory, publish_file, remove_file};
    use crate::tool::builtin::common::content_hash;

    #[test]
    fn publication_and_removal_follow_the_bound_directory() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let original = root.join("approved");
        let moved = root.join("moved");
        fs::create_dir(&original).unwrap();
        fs::write(original.join("note.txt"), "original").unwrap();
        let directory = BoundDirectory::open(&original).unwrap();

        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("note.txt"), "replacement").unwrap();

        let name = OsStr::new("note.txt");
        assert!(
            publish_file(
                &directory,
                name,
                Some(&content_hash(b"original")),
                b"updated",
                u64::MAX,
                &|| false,
            )
            .is_ok()
        );
        assert_eq!(fs::read(moved.join(name)).unwrap(), b"updated");
        assert!(
            remove_file(
                &directory,
                name,
                &content_hash(b"updated"),
                u64::MAX,
                &|| false,
            )
            .is_ok()
        );
        assert!(!moved.join(name).exists());
        assert_eq!(fs::read(original.join(name)).unwrap(), b"replacement");
    }
}
