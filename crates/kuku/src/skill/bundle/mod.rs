use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::bufread::GzDecoder;

use super::definition::{SkillDefinition, SkillSource};
use format::{
    FileEntry, HEADER, MAX_CONTENT_BYTES, MAX_FILE_BYTES, MAX_MANIFEST_BYTES, Manifest,
    fingerprint, valid_path,
};

mod format;
pub(crate) mod resources;
#[cfg(test)]
mod tests;

pub(crate) const NAME: &str = "kuku-docs";
pub(crate) const CURRENT: &str = env!("KUKU_DOCS_FINGERPRINT");
const CONTENT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/kuku-docs.bundle"));
const INSTRUCTIONS: &str = include_str!("../../../skills/kuku-docs/SKILL.md");
const MANIFEST: &str = "manifest.json";
const RESOURCE_ROOT: &str = "references/docs";
const MAX_ARCHIVE_BYTES: usize = MAX_CONTENT_BYTES + MAX_MANIFEST_BYTES + (1 << 20);

pub(super) fn definition() -> SkillDefinition {
    let mut definition =
        super::loader::parse_skill(INSTRUCTIONS, Path::new(NAME), &SkillSource::Builtin)
            .expect("embedded kuku-docs SKILL.md is valid");
    definition.source_path = None;
    definition
}

/// Save the compressed source before a loaded Skill becomes part of a Turn.
pub(crate) async fn retain(home: &Path, version: &str) -> io::Result<()> {
    let home = home.to_path_buf();
    let version = version.to_owned();
    // The blocking worker can observe the caller dropping this future.
    let (cancel, waiting) = tokio::sync::oneshot::channel::<()>();
    let result = tokio::task::spawn_blocking(move || {
        let archive = retain_archive(&home, &version, &|| cancel.is_closed())?;
        crate::util::fs_sync::sync_parent(&archive)?;
        #[cfg(unix)]
        for directory in archive.ancestors().skip(1).take(2) {
            crate::util::fs_sync::sync_parent(directory)?;
        }
        Ok(())
    })
    .await
    .map_err(io::Error::other)?;
    drop(waiting);
    result
}

pub(crate) fn retain_cancellable(
    home: &Path,
    version: &str,
    cancelled: &impl Fn() -> bool,
) -> io::Result<()> {
    retain_archive(home, version, cancelled).map(drop)
}

fn retain_archive(
    home: &Path,
    version: &str,
    cancelled: &impl Fn() -> bool,
) -> io::Result<PathBuf> {
    ensure_active(cancelled)?;
    let path = version_path(home, version)?;
    let archive = path.with_extension("bundle");
    if version != CURRENT {
        let bytes = read_regular(&archive, MAX_ARCHIVE_BYTES)?;
        archive_parts(&bytes, version)?;
        return Ok(archive);
    }
    if read_regular(&archive, MAX_ARCHIVE_BYTES).is_ok_and(|bytes| bytes == CONTENT) {
        return Ok(archive);
    }
    let _lock = lock(&path, cancelled)?;
    if read_regular(&archive, MAX_ARCHIVE_BYTES).is_ok_and(|bytes| bytes == CONTENT) {
        return Ok(archive);
    }
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().expect("cache has a parent"))?;
    staged.write_all(CONTENT)?;
    staged.as_file().sync_all()?;
    ensure_active(cancelled)?;
    staged.persist(&archive).map_err(|error| error.error)?;
    sync_cache_parent(&archive)?;
    Ok(archive)
}

struct Cache {
    path: PathBuf,
    manifest: Manifest,
}

impl Cache {
    fn open(path: &Path, version: &str) -> io::Result<Self> {
        require_directory(path)?;
        require_directory(&path.join("references"))?;
        require_directory(&path.join(RESOURCE_ROOT))?;
        let index = read_regular(&path.join(MANIFEST), MAX_MANIFEST_BYTES)?;
        let manifest = parse_manifest(&index, version)?;
        Ok(Self {
            path: path.to_path_buf(),
            manifest,
        })
    }

    fn file(&self, relative: &str, entry: &FileEntry) -> io::Result<Vec<u8>> {
        let mut path = self.path.join(RESOURCE_ROOT);
        require_directory(&self.path.join("references"))?;
        require_directory(&path)?;
        let (directories, file) = relative.rsplit_once('/').unwrap_or(("", relative));
        if !directories.is_empty() {
            for part in directories.split('/') {
                path.push(part);
                require_directory(&path)?;
            }
        }
        let bytes = read_regular(&path.join(file), entry.bytes)?;
        if bytes.len() != entry.bytes || fingerprint(&bytes) != entry.sha256 {
            return Err(invalid("Skill resource file failed integrity check"));
        }
        Ok(bytes)
    }
}

fn with_cache<T>(
    home: &Path,
    version: &str,
    cancelled: &impl Fn() -> bool,
    read: impl Fn(&Cache) -> io::Result<T>,
) -> io::Result<T> {
    ensure_active(cancelled)?;
    let path = version_path(home, version)?;
    if let Some(result) = try_cache(&path, version, &read)? {
        return Ok(result);
    }
    let _lock = lock(&path, cancelled)?;
    if let Some(result) = try_cache(&path, version, &read)? {
        return Ok(result);
    }
    publish(&path, version, cancelled)?;
    read(&Cache::open(&path, version)?)
}

fn try_cache<T>(
    path: &Path,
    version: &str,
    read: &impl Fn(&Cache) -> io::Result<T>,
) -> io::Result<Option<T>> {
    match Cache::open(path, version).and_then(|cache| read(&cache)) {
        Ok(result) => Ok(Some(result)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidData
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn publish(path: &Path, version: &str, cancelled: &impl Fn() -> bool) -> io::Result<()> {
    let retained;
    let bytes = if version == CURRENT {
        CONTENT
    } else {
        retained = read_regular(&path.with_extension("bundle"), MAX_ARCHIVE_BYTES)?;
        &retained
    };
    let (index, compressed) = archive_parts(bytes, version)?;
    let manifest = parse_manifest(index, version)?;
    let parent = path.parent().expect("cache version has a parent");
    let prefix = format!(".extract-{version}-");
    // This version's interprocess lock also covers staging left by a killed process.
    for entry in fs::read_dir(parent)? {
        ensure_active(cancelled)?;
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) && entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }
    let staged = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(parent)?;
    let version_dir = staged.path().join("version");
    let docs = version_dir.join(RESOURCE_ROOT);
    fs::create_dir_all(&docs)?;
    unpack(&docs, manifest, compressed, cancelled)?;
    File::create(version_dir.join(MANIFEST)).and_then(|mut file| {
        file.write_all(index)?;
        file.sync_all()
    })?;
    sync_tree(&version_dir)?;
    ensure_active(cancelled)?;
    match fs::symlink_metadata(path) {
        Ok(_) => fs::rename(path, staged.path().join("invalid"))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(&version_dir, path)?;
    sync_cache_parent(path)
}

fn unpack(
    docs: &Path,
    manifest: Manifest,
    compressed: &[u8],
    cancelled: &impl Fn() -> bool,
) -> io::Result<()> {
    let mut reader = GzDecoder::new(compressed);
    for (relative, entry) in manifest.files {
        ensure_active(cancelled)?;
        let mut contents = vec![0; entry.bytes];
        reader.read_exact(&mut contents)?;
        if fingerprint(&contents) != entry.sha256 {
            return Err(invalid("compressed Skill resource failed integrity check"));
        }
        let path = docs.join(relative);
        fs::create_dir_all(path.parent().expect("resource file has a parent"))?;
        let mut file = File::create(path)?;
        file.write_all(&contents)?;
        file.sync_all()?;
    }
    if reader.read(&mut [0])? != 0 || !reader.into_inner().is_empty() {
        return Err(invalid("extra bytes in Skill resource bundle"));
    }
    Ok(())
}

fn archive_parts<'a>(bytes: &'a [u8], version: &str) -> io::Result<(&'a [u8], &'a [u8])> {
    let bytes = bytes
        .strip_prefix(HEADER)
        .ok_or_else(|| invalid("invalid Skill resource header"))?;
    let (length, bytes) = bytes
        .split_first_chunk::<4>()
        .ok_or_else(|| invalid("missing Skill resource manifest"))?;
    let length = u32::from_be_bytes(*length) as usize;
    if length > MAX_MANIFEST_BYTES {
        return Err(invalid("Skill resource manifest exceeds limit"));
    }
    let (index, compressed) = bytes
        .split_at_checked(length)
        .ok_or_else(|| invalid("incomplete Skill resource manifest"))?;
    if fingerprint(index) != version {
        return Err(invalid("Skill resource version failed integrity check"));
    }
    Ok((index, compressed))
}

fn parse_manifest(index: &[u8], version: &str) -> io::Result<Manifest> {
    if fingerprint(index) != version {
        return Err(invalid("Skill resource manifest failed integrity check"));
    }
    let manifest: Manifest = serde_json::from_slice(index).map_err(io::Error::other)?;
    let mut total = 0_usize;
    for (path, entry) in &manifest.files {
        if !valid_path(path) || entry.bytes > MAX_FILE_BYTES || !is_hex(&entry.sha256, 64) {
            return Err(invalid("invalid entry in Skill resource manifest"));
        }
        total = total
            .checked_add(entry.bytes)
            .filter(|total| *total <= MAX_CONTENT_BYTES)
            .ok_or_else(|| invalid("Skill resource manifest exceeds content limit"))?;
    }
    if !manifest.source_revision.is_empty() && !is_hex(&manifest.source_revision, 40) {
        return Err(invalid("invalid revision in Skill resource manifest"));
    }
    Ok(manifest)
}

fn version_path(home: &Path, version: &str) -> io::Result<PathBuf> {
    if !is_hex(version, 64) {
        return Err(invalid("invalid Skill resource version"));
    }
    let mut path = home.to_path_buf();
    for part in ["skill-resources", NAME] {
        path.push(part);
        match fs::create_dir(&path) {
            Ok(()) => sync_cache_parent(&path)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => require_directory(&path)?,
            Err(error) => return Err(error),
        }
    }
    Ok(path.join(version))
}

fn lock(path: &Path, cancelled: &impl Fn() -> bool) -> io::Result<fslock::LockFile> {
    let mut lock = fslock::LockFile::open(&path.with_extension("lock"))?;
    loop {
        ensure_active(cancelled)?;
        if lock.try_lock()? {
            return Ok(lock);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn ensure_active(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Skill resource access cancelled",
        ))
    } else {
        Ok(())
    }
}

fn read_regular(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() > max_bytes as u64 {
        return Err(invalid("invalid cached Skill resource file"));
    }
    let mut contents = Vec::new();
    File::open(path)?
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut contents)?;
    if contents.len() > max_bytes {
        return Err(invalid("Skill resource exceeds content limit"));
    }
    Ok(contents)
}

fn require_directory(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(invalid("invalid cached Skill resource directory"));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_tree(path: &Path) -> io::Result<()> {
    for child in fs::read_dir(path)? {
        let child = child?;
        if child.file_type()?.is_dir() {
            sync_tree(&child.path())?;
        }
    }
    crate::util::fs_sync::sync_directory(path)
}

#[cfg(not(unix))]
fn sync_tree(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_cache_parent(path: &Path) -> io::Result<()> {
    crate::util::fs_sync::sync_parent(path)
}

#[cfg(not(unix))]
fn sync_cache_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
