use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, Metadata};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::execution::{FileCommitState, FileMutationFacts};
use crate::tool::{ToolErrorReason, ToolResultEnvelope};
use crate::util::fs_sync;

const HASH_BUFFER_BYTES: usize = 64 * 1024;
const MUTATION_LOCKS: usize = 64;
const MUTATION_PREVIEW_BYTES: usize = 80_000;

static MUTATIONS: [Mutex<()>; MUTATION_LOCKS] = [const { Mutex::new(()) }; MUTATION_LOCKS];

pub(crate) struct StableFile {
    file: File,
    opened: Metadata,
}

impl StableFile {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
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
        Ok(Self { file, opened })
    }

    pub(crate) fn verify(&self, path: &Path) -> io::Result<()> {
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

impl Read for StableFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
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

pub(crate) fn read_file_as_utf8(path: &Path) -> Result<String, ToolResultEnvelope> {
    let mut content = String::new();
    let read = (|| -> io::Result<()> {
        let mut file = StableFile::open(path)?;
        file.read_to_string(&mut content)?;
        file.verify(path)
    })();
    read.map_err(|error| file_read_error(path, error))?;
    Ok(content)
}

pub(crate) fn file_version(path: &Path) -> io::Result<(String, Metadata)> {
    let mut file = StableFile::open(path)?;
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
    let summary = format!("failed: {error}");
    let detail = format!("{}: {error}", path.display());
    if error.kind() == io::ErrorKind::WouldBlock {
        ToolResultEnvelope::error_with_reason(summary, detail, ToolErrorReason::SnapshotStale)
    } else {
        ToolResultEnvelope::error(summary, detail)
    }
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut staged = stage_file(path, bytes)?;
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
) -> Result<Publication, WorkspaceCommitError> {
    let lock = lock_for_path(path);
    let _commit = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    verify_canonical_parent(path).map_err(version_error)?;
    let mut staged = stage_file(path, bytes)?;
    staged.flush()?;
    if expected.is_some() {
        let metadata = fs::symlink_metadata(path).map_err(version_error)?;
        staged.as_file().set_permissions(metadata.permissions())?;
    }
    fs_sync::sync_file(staged.as_file())?;
    let staged = staged.into_temp_path();
    verify_canonical_parent(path).map_err(version_error)?;
    if let Some(expected) = expected {
        verify_file_version(path, expected)?;
        return replace_existing(&staged, path);
    }
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
) -> Result<Publication, WorkspaceCommitError> {
    let lock = lock_for_path(path);
    let _commit = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    verify_canonical_parent(path).map_err(version_error)?;
    verify_file_version(path, expected)?;
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

fn stage_file(path: &Path, bytes: &[u8]) -> io::Result<tempfile::NamedTempFile> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    let mut staged = tempfile::Builder::new()
        .prefix(".kuku-")
        .tempfile_in(parent)?;
    staged.write_all(bytes)?;
    Ok(staged)
}

fn lock_for_path(path: &Path) -> &'static Mutex<()> {
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    &MUTATIONS[(hash.finish() % MUTATION_LOCKS as u64) as usize]
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

fn verify_file_version(path: &Path, expected: &str) -> Result<(), WorkspaceCommitError> {
    let (current, _) = file_version(path).map_err(version_error)?;
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
                    publish_file(path, Some(version), bytes)
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
            publish_file(&path, None, b"replacement"),
            Err(WorkspaceCommitError::Conflict)
        ));
        assert!(matches!(
            remove_observed_file(&path, &content_hash(b"old")),
            Err(WorkspaceCommitError::Conflict)
        ));
        assert_eq!(b"current", fs::read(path).unwrap().as_slice());
    }
}
