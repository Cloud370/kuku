use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, Metadata};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::execution::{FileCommitState, FileMutationFacts};
use crate::tool::{ToolErrorReason, ToolResultEnvelope};
use crate::util::fs_sync;

const HASH_BUFFER_BYTES: usize = 64 * 1024;
const MUTATION_LOCKS: usize = 64;
const MUTATION_PREVIEW_BYTES: usize = 80_000;
const MUTATION_LOCK_RETRY: Duration = Duration::from_millis(10);

static MUTATIONS: [Mutex<()>; MUTATION_LOCKS] = [const { Mutex::new(()) }; MUTATION_LOCKS];

pub(crate) type FileCancellation<'a> = dyn Fn() -> bool + Sync + 'a;

#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("file operation cancelled")
    }
}

impl std::error::Error for Cancelled {}

pub(crate) struct StableFile<'a> {
    file: File,
    opened: Metadata,
    cancelled: &'a FileCancellation<'a>,
}

impl<'a> StableFile<'a> {
    pub(crate) fn open(path: &Path, cancelled: &'a FileCancellation<'a>) -> io::Result<Self> {
        ensure_active(cancelled)?;
        verify_canonical_parent(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not a file",
            ));
        }
        let file = open_regular_file(path)?;
        let opened = file.metadata()?;
        if !same_version_marker(&metadata, &opened) {
            return Err(changed_during_read());
        }
        Ok(Self {
            file,
            opened,
            cancelled,
        })
    }

    pub(crate) fn verify(&self, path: &Path) -> io::Result<()> {
        ensure_active(self.cancelled)?;
        verify_canonical_parent(path)?;
        let current = fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                changed_during_read()
            } else {
                error
            }
        })?;
        if !same_version_marker(&self.opened, &current)
            || !same_version_marker(&self.opened, &self.file.metadata()?)
        {
            return Err(changed_during_read());
        }
        Ok(())
    }
}

impl Read for StableFile<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        ensure_active(self.cancelled)?;
        self.file.read(buffer)
    }
}

pub(crate) fn content_hash(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    format_content_hash(hash)
}

pub(crate) fn format_content_hash(hash: Sha256) -> String {
    format!("sha256:{:x}", hash.finalize())
}

pub(crate) fn read_file_as_utf8(
    path: &Path,
    cancelled: &FileCancellation<'_>,
) -> Result<String, ToolResultEnvelope> {
    let mut content = String::new();
    let read = (|| -> io::Result<()> {
        let mut file = StableFile::open(path, cancelled)?;
        file.read_to_string(&mut content)?;
        file.verify(path)
    })();
    read.map_err(|error| file_read_error(path, error))?;
    Ok(content)
}

pub(crate) fn file_version(
    path: &Path,
    cancelled: &FileCancellation<'_>,
) -> io::Result<(String, Metadata)> {
    let mut file = StableFile::open(path, cancelled)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; HASH_BUFFER_BYTES];
    loop {
        let count = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    file.verify(path)?;
    Ok((format_content_hash(hash), file.opened))
}

pub(crate) fn file_read_error(path: &Path, error: io::Error) -> ToolResultEnvelope {
    if is_cancellation(&error) {
        return ToolResultEnvelope::cancelled(format!(
            "file operation cancelled: {}",
            path.display()
        ));
    }
    let summary = format!("failed: {error}");
    let detail = format!("{}: {error}", path.display());
    if error.kind() == io::ErrorKind::WouldBlock {
        ToolResultEnvelope::error_with_reason(summary, detail, ToolErrorReason::SnapshotStale)
    } else {
        ToolResultEnvelope::error(summary, detail)
    }
}

pub(crate) fn ensure_active(cancelled: &FileCancellation<'_>) -> io::Result<()> {
    if cancelled() {
        return Err(io::Error::other(Cancelled));
    }
    Ok(())
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut staged = stage_file(path, bytes, &|| false)?;
    staged.flush()?;
    fs_sync::sync_file(staged.as_file())?;
    let staged = staged.into_temp_path();
    fs_sync::rename(&staged, path)
}

pub(crate) enum WorkspaceCommitError {
    Conflict,
    WriteFailed(io::Error),
}

impl WorkspaceCommitError {
    pub(crate) fn result(self, relative: &str) -> ToolResultEnvelope {
        match self {
            Self::Conflict => ToolResultEnvelope::error_with_reason(
                format!("failed: {relative} changed before committing"),
                format!("file changed; read {relative} again before modifying"),
                ToolErrorReason::SnapshotStale,
            ),
            Self::WriteFailed(error) if is_cancellation(&error) => {
                ToolResultEnvelope::cancelled(format!("file operation cancelled: {relative}"))
            }
            Self::WriteFailed(error) => ToolResultEnvelope::error(
                format!("failed: {error}"),
                format!("error modifying file: {relative}"),
            ),
        }
    }
}

impl From<io::Error> for WorkspaceCommitError {
    fn from(error: io::Error) -> Self {
        Self::WriteFailed(error)
    }
}

pub(crate) enum Publication {
    Committed,
    Unconfirmed(io::Error),
}

impl Publication {
    pub(crate) fn result(
        self,
        mut facts: FileMutationFacts,
        summary: String,
        fields: serde_json::Value,
    ) -> ToolResultEnvelope {
        if matches!(self, Self::Unconfirmed(_)) {
            facts.commit_state = FileCommitState::PublishedUnconfirmed;
        }
        let mut structured = serde_json::to_value(facts).unwrap();
        if let (Some(result), serde_json::Value::Object(fields)) =
            (structured.as_object_mut(), fields)
        {
            result.extend(fields);
        }
        match self {
            Self::Committed => ToolResultEnvelope::ok(summary.clone(), summary, structured),
            Self::Unconfirmed(error) => {
                let mut result = ToolResultEnvelope::error(
                    summary,
                    format!(
                        "the file change was applied; durability could not be confirmed: {error}"
                    ),
                );
                result.structured = Some(structured);
                result
            }
        }
    }
}

pub(crate) fn publish_file(
    path: &Path,
    expected: Option<&str>,
    bytes: &[u8],
    cancelled: &FileCancellation<'_>,
) -> Result<Publication, WorkspaceCommitError> {
    let _commit = path_lock(path, cancelled)?;
    match verify_canonical_parent(path) {
        Err(error) if expected.is_none() && error.kind() == io::ErrorKind::NotFound => {
            create_missing_parents(path, cancelled).map_err(version_error)?;
        }
        result => result.map_err(version_error)?,
    }
    let mut staged = stage_file(path, bytes, cancelled)?;
    staged.flush()?;
    if expected.is_some() {
        let metadata = fs::symlink_metadata(path).map_err(version_error)?;
        staged.as_file().set_permissions(metadata.permissions())?;
    }
    fs_sync::sync_file(staged.as_file())?;
    let staged = staged.into_temp_path();
    verify_canonical_parent(path).map_err(version_error)?;
    if let Some(expected) = expected {
        verify_file_version(path, expected, cancelled)?;
        ensure_active(cancelled)?;
        return replace_existing(&staged, path);
    }
    ensure_active(cancelled)?;
    staged.persist_noclobber(path).map_err(|error| {
        if error.error.kind() == io::ErrorKind::AlreadyExists {
            WorkspaceCommitError::Conflict
        } else {
            WorkspaceCommitError::WriteFailed(error.error)
        }
    })?;
    Ok(confirm_publication(fs_sync::sync_parent(path)))
}

pub(crate) fn remove_observed_file(
    path: &Path,
    expected: &str,
    cancelled: &FileCancellation<'_>,
) -> Result<Publication, WorkspaceCommitError> {
    let _commit = path_lock(path, cancelled)?;
    verify_canonical_parent(path).map_err(version_error)?;
    verify_file_version(path, expected, cancelled)?;
    ensure_active(cancelled)?;
    fs::remove_file(path).map_err(version_error)?;
    #[cfg(unix)]
    return Ok(confirm_publication(fs_sync::sync_parent(path)));
    #[cfg(not(unix))]
    Ok(Publication::Committed)
}

pub(crate) fn mutation_preview(content: &str, fields: &mut serde_json::Value) {
    let end = content.floor_char_boundary(MUTATION_PREVIEW_BYTES.min(content.len()));
    fields["raw_text_after"] = content[..end].into();
    if end != content.len() {
        fields["raw_text_after_truncated"] = true.into();
    }
}

fn same_version_marker(left: &Metadata, right: &Metadata) -> bool {
    if !right.is_file()
        || left.len() != right.len()
        || left.modified().ok() != right.modified().ok()
        || left.created().ok() != right.created().ok()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (left.dev(), left.ino(), left.ctime(), left.ctime_nsec())
            == (right.dev(), right.ino(), right.ctime(), right.ctime_nsec())
    }
    #[cfg(not(unix))]
    true
}

fn changed_during_read() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "file changed while it was being read",
    )
}

fn is_cancellation(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|cause| cause.is::<Cancelled>())
}

fn open_regular_file(path: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

fn stage_file(
    path: &Path,
    bytes: &[u8],
    cancelled: &FileCancellation<'_>,
) -> io::Result<tempfile::NamedTempFile> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    let mut staged = tempfile::Builder::new()
        .prefix(".kuku-")
        .tempfile_in(parent)?;
    for chunk in bytes.chunks(HASH_BUFFER_BYTES) {
        ensure_active(cancelled)?;
        staged.write_all(chunk)?;
    }
    Ok(staged)
}

fn path_lock(path: &Path, cancelled: &FileCancellation<'_>) -> io::Result<MutexGuard<'static, ()>> {
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    let lock = &MUTATIONS[(hash.finish() % MUTATION_LOCKS as u64) as usize];
    loop {
        ensure_active(cancelled)?;
        match lock.try_lock() {
            Ok(held) => return Ok(held),
            Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(MUTATION_LOCK_RETRY),
        }
    }
}

fn verify_canonical_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    if parent.canonicalize()? != parent {
        return Err(changed_during_read());
    }
    Ok(())
}

fn create_missing_parents(path: &Path, cancelled: &FileCancellation<'_>) -> io::Result<()> {
    let mut missing = Vec::new();
    for directory in path.ancestors().skip(1) {
        match directory.canonicalize() {
            Ok(canonical) if canonical == directory => break,
            Ok(_) => return Err(changed_during_read()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => missing.push(directory),
            Err(error) => return Err(error),
        }
    }

    for directory in missing.into_iter().rev() {
        ensure_active(cancelled)?;
        verify_canonical_parent(directory)?;
        match fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        if directory.canonicalize()? != directory {
            return Err(changed_during_read());
        }
        if !fs::metadata(directory)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", directory.display()),
            ));
        }
        #[cfg(unix)]
        fs_sync::sync_parent(directory)?;
    }
    verify_canonical_parent(path)
}

fn verify_file_version(
    path: &Path,
    expected: &str,
    cancelled: &FileCancellation<'_>,
) -> Result<(), WorkspaceCommitError> {
    let (current, _) = file_version(path, cancelled).map_err(version_error)?;
    if current != expected {
        return Err(WorkspaceCommitError::Conflict);
    }
    Ok(())
}

fn version_error(error: io::Error) -> WorkspaceCommitError {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::WouldBlock | io::ErrorKind::InvalidInput
    ) {
        WorkspaceCommitError::Conflict
    } else {
        WorkspaceCommitError::WriteFailed(error)
    }
}

fn confirm_publication(result: io::Result<()>) -> Publication {
    match result {
        Ok(()) => Publication::Committed,
        Err(error) => Publication::Unconfirmed(error),
    }
}

#[cfg(unix)]
fn replace_existing(staged: &Path, path: &Path) -> Result<Publication, WorkspaceCommitError> {
    fs::rename(staged, path)?;
    Ok(confirm_publication(fs_sync::sync_parent(path)))
}

#[cfg(not(unix))]
fn replace_existing(staged: &Path, path: &Path) -> Result<Publication, WorkspaceCommitError> {
    fs_sync::rename(staged, path)?;
    Ok(Publication::Committed)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn concurrent_publications_cannot_both_commit_the_same_version() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().canonicalize().unwrap().join("file.txt");
        fs::write(&path, b"original").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let version = content_hash(b"original");
        let (alpha, beta) = std::thread::scope(|scope| {
            let attempt = |bytes: &'static [u8]| {
                let barrier = barrier.clone();
                let path = &path;
                let version = &version;
                scope.spawn(move || {
                    barrier.wait();
                    publish_file(path, Some(version), bytes, &|| false)
                })
            };
            let alpha = attempt(b"alpha");
            let beta = attempt(b"beta");
            (alpha.join().unwrap(), beta.join().unwrap())
        });
        assert!(matches!(
            (&alpha, &beta),
            (Ok(_), Err(WorkspaceCommitError::Conflict))
                | (Err(WorkspaceCommitError::Conflict), Ok(_))
        ));
        let expected: &[u8] = if alpha.is_ok() { b"alpha" } else { b"beta" };
        assert_eq!(expected, fs::read(path).unwrap());
    }

    #[test]
    fn creation_and_removal_refuse_to_clobber_a_different_file() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().canonicalize().unwrap().join("file.txt");
        fs::write(&path, b"current").unwrap();
        assert!(matches!(
            publish_file(&path, None, b"replacement", &|| false),
            Err(WorkspaceCommitError::Conflict)
        ));
        assert!(matches!(
            remove_observed_file(&path, &content_hash(b"old"), &|| false),
            Err(WorkspaceCommitError::Conflict)
        ));
        assert_eq!(b"current", fs::read(path).unwrap().as_slice());
    }

    #[test]
    fn cancellation_after_staging_prevents_publication_and_cleans_up() {
        let temporary = tempfile::tempdir().unwrap();
        let dir = temporary.path().canonicalize().unwrap();
        let path = dir.join("new.txt");
        let cancelled = || fs::read_dir(&dir).unwrap().next().is_some();

        let error = publish_file(&path, None, b"", &cancelled)
            .err()
            .expect("cancelled before the staged file can be published");

        assert_eq!("cancelled", error.result("new.txt").status);
        assert!(fs::read_dir(dir).unwrap().next().is_none());
    }

    #[test]
    fn version_is_revalidated_after_staging_and_before_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let dir = temporary.path().canonicalize().unwrap();
        let path = dir.join("file.txt");
        fs::write(&path, b"original").unwrap();
        let external_write = AtomicBool::new(false);
        let changed_after_staging = || {
            if fs::read_dir(&dir).unwrap().count() > 1
                && !external_write.swap(true, Ordering::Relaxed)
            {
                fs::write(&path, b"external").unwrap();
            }
            false
        };

        let result = publish_file(
            &path,
            Some(&content_hash(b"original")),
            b"replacement",
            &changed_after_staging,
        );

        assert!(matches!(result, Err(WorkspaceCommitError::Conflict)));
        assert_eq!(b"external", fs::read(&path).unwrap().as_slice());
        assert_eq!(1, fs::read_dir(dir).unwrap().count());
    }
}
