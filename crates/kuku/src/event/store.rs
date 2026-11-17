//! Durable append-only session event storage.
//! Open log paths must remain stable during a writer's lifetime.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};

use tokio::sync::Notify;

use crate::error::{Error, Result};
use crate::util::fs_sync;

use super::types::{EventPayload, StoredEvent};

mod codec;
mod reader;

use codec::serialize_record;
use reader::EventReader;

const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_PAGE_BYTES: usize = 1024 * 1024;
const MAX_PAGE_RECORDS: usize = 256;
const INDEX_STRIDE: usize = 128;

type StoreRegistry = Mutex<HashMap<PathBuf, StoreEntry>>;

#[derive(Clone, Copy)]
struct EventOffset {
    id: u64,
    offset: u64,
}

struct StoreInner {
    path: PathBuf,
    writer: Mutex<Writer>,
    changed: Arc<Notify>,
    release: Arc<WriterRelease>,
}

impl Drop for StoreInner {
    fn drop(&mut self) {
        let writer = self
            .writer
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(writer.file.take());
        drop(writer.file_lock.take());
        self.release.finish();
    }
}

struct StoreEntry {
    writer: Weak<StoreInner>,
    release: Arc<WriterRelease>,
}

#[derive(Default)]
struct WriterRelease {
    finished: Mutex<bool>,
    changed: Condvar,
}

impl WriterRelease {
    fn finish(&self) {
        *self.finished.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn is_finished(&self) -> bool {
        *self.finished.lock().unwrap()
    }

    fn wait(&self) {
        let mut finished = self.finished.lock().unwrap();
        while !*finished {
            finished = self.changed.wait(finished).unwrap();
        }
    }
}

struct Writer {
    file: Option<File>,
    file_lock: Option<fslock::LockFile>,
    index: Vec<EventOffset>,
    event_count: usize,
    committed_len: u64,
    last_id: u64,
    poison: Option<String>,
}

impl Writer {
    fn open(path: &Path, mut file: File) -> Result<Self> {
        let file_lock = acquire_file_lock(path)?;
        fs_sync::lock_writer(&file, path)?;
        let mut reader = EventReader::new(BufReader::new(&mut file), path, 0);
        let mut index = Vec::new();
        let mut event_count = 0_usize;
        let mut last_id = 0;
        while let Some(record) = reader.next()? {
            if event_count.is_multiple_of(INDEX_STRIDE) {
                index.push(EventOffset {
                    id: record.event.id,
                    offset: record.offset,
                });
            }
            event_count += 1;
            last_id = record.event.id;
        }
        let committed_len = reader.committed_len();
        let incomplete_tail = reader.incomplete_tail();
        drop(reader);
        if incomplete_tail {
            file.set_len(committed_len)?;
            file.sync_data()?;
        }
        file.sync_all()?;
        fs_sync::sync_parent(path)?;
        Ok(Self {
            file: Some(file),
            file_lock: Some(file_lock),
            index,
            event_count,
            committed_len,
            last_id,
            poison: None,
        })
    }

    fn ensure_healthy(&self, path: &Path) -> Result<()> {
        if let Some(reason) = &self.poison {
            return Err(io::Error::other(format!(
                "event writer {} stopped after a durability failure: {reason}",
                path.display()
            ))
            .into());
        }
        Ok(())
    }

    fn commit(&mut self, path: &Path, event_id: u64, record: &[u8]) -> Result<()> {
        let file = self.file.as_mut().unwrap();
        if let Err(error) = file
            .seek(SeekFrom::Start(self.committed_len))
            .and_then(|_| file.write_all(record))
            .and_then(|()| fs_sync::sync_file(file))
        {
            self.poison = Some(error.to_string());
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "cannot commit event {event_id} to {}: {error}",
                    path.display()
                ),
            )
            .into());
        }
        if self.event_count.is_multiple_of(INDEX_STRIDE) {
            self.index.push(EventOffset {
                id: event_id,
                offset: self.committed_len,
            });
        }
        self.event_count += 1;
        self.committed_len += record.len() as u64;
        self.last_id = event_id;
        Ok(())
    }

    fn replay(&mut self, path: &Path) -> Result<Vec<StoredEvent>> {
        self.ensure_healthy(path)?;
        let file = self.file.as_mut().unwrap();
        file.seek(SeekFrom::Start(0))?;
        read_all(file.take(self.committed_len), path)
    }

    fn read_after(&mut self, path: &Path, after: u64, limit: usize) -> Result<Vec<StoredEvent>> {
        self.ensure_healthy(path)?;
        if limit == 0 || after >= self.last_id {
            return Ok(Vec::new());
        }
        let checkpoint = self
            .index
            .partition_point(|entry| entry.id <= after)
            .saturating_sub(1);
        let start = self.index[checkpoint].offset;
        let file = self.file.as_mut().unwrap();
        file.seek(SeekFrom::Start(start))?;
        let file = file.take(self.committed_len - start);
        read_page_from(file, path, start, after, limit)
    }
}

/// Append-only event storage whose clones share one durable writer for a path.
#[derive(Clone)]
pub struct EventStore {
    inner: Arc<StoreInner>,
}

impl EventStore {
    /// Open a shared writer, repairing only an incomplete final record.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let absolute = std::env::current_dir()?.join(path);
        let mut stores = registry().lock().unwrap();
        if let Ok(key) = fs::canonicalize(&absolute) {
            if let Some(inner) = find_active_store(&stores, &key) {
                return Ok(Self { inner });
            }
        }
        if let Some(parent) = absolute.parent() {
            fs_sync::create_dir_all(parent)?;
        }
        let key = canonical_store_path(&absolute)?;
        if let Some(inner) = find_active_store(&stores, &key) {
            return Ok(Self { inner });
        }
        let file = fs_sync::write_options()
            .read(true)
            .create(true)
            .truncate(false)
            .open(&key)?;
        let path = fs::canonicalize(key)?;
        if let Some(inner) = find_active_store(&stores, &path) {
            return Ok(Self { inner });
        }
        let writer = Writer::open(&path, file)?;
        let release = Arc::new(WriterRelease::default());
        let inner = Arc::new(StoreInner {
            path: path.clone(),
            writer: Mutex::new(writer),
            changed: Arc::new(Notify::new()),
            release: Arc::clone(&release),
        });
        stores.retain(|_, store| !store.release.is_finished());
        stores.insert(
            path,
            StoreEntry {
                writer: Arc::downgrade(&inner),
                release,
            },
        );
        Ok(Self { inner })
    }

    /// Append an event and return its assigned ID after its durability barrier succeeds.
    pub fn append(&mut self, mut payload: EventPayload) -> Result<StoredEvent> {
        let mut writer = self.inner.writer.lock().unwrap();
        writer.ensure_healthy(&self.inner.path)?;
        let id = writer
            .last_id
            .checked_add(1)
            .ok_or_else(|| Error::InvalidEventStream("event IDs exhausted".to_string()))?;
        patch_event_id(&mut payload, id)?;
        let event = StoredEvent { id, payload };
        let record = serialize_record(&event)?;
        writer.commit(&self.inner.path, id, &record)?;
        drop(writer);
        self.changed().notify_waiters();
        Ok(event)
    }

    /// Read committed events after an ID, capped at 256 records and a 1 MiB page target.
    pub fn read_after(&self, after: u64, limit: usize) -> Result<Vec<StoredEvent>> {
        self.inner
            .writer
            .lock()
            .unwrap()
            .read_after(&self.inner.path, after, limit)
    }

    /// Read a bounded committed page without opening a writer or repairing the file.
    pub fn read_page(path: impl AsRef<Path>, after: u64, limit: usize) -> Result<Vec<StoredEvent>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let path = match fs::canonicalize(path) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            return inner.writer.lock().unwrap().read_after(&path, after, limit);
        }
        let _file_lock = acquire_file_lock(&path)?;
        let file = File::open(&path)?;
        fs_sync::lock_reader(&file, &path)?;
        let result = read_page_from(file, &path, 0, after, limit);
        drop(stores);
        result
    }

    /// Return the last durably committed event ID, or zero for an empty store.
    pub fn last_id(&self) -> u64 {
        self.inner.writer.lock().unwrap().last_id
    }

    /// Read all committed events without opening another writer or repairing the file.
    pub fn replay(path: impl AsRef<Path>) -> Result<Vec<StoredEvent>> {
        let path = match fs::canonicalize(path) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            return inner.writer.lock().unwrap().replay(&path);
        }
        let _file_lock = acquire_file_lock(&path)?;
        let file = File::open(&path)?;
        fs_sync::lock_reader(&file, &path)?;
        let result = read_all(file, &path);
        drop(stores);
        result
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.last_id().saturating_add(1)
    }

    pub(crate) fn changed(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.changed)
    }
}

fn registry() -> &'static StoreRegistry {
    static STORES: OnceLock<StoreRegistry> = OnceLock::new();
    STORES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn find_active_store(
    stores: &HashMap<PathBuf, StoreEntry>,
    path: &Path,
) -> Option<Arc<StoreInner>> {
    let entry = stores.get(path)?;
    if let Some(inner) = entry.writer.upgrade() {
        return Some(inner);
    }
    // NOTE: An expired Weak may still be releasing the operating-system lock.
    entry.release.wait();
    None
}

fn canonical_store_path(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().unwrap();
            Ok(fs::canonicalize(parent)?.join(path.file_name().unwrap()))
        }
        Err(error) => Err(error.into()),
    }
}

fn acquire_file_lock(path: &Path) -> Result<fslock::LockFile> {
    let mut lock_name = path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let mut lock = fslock::LockFile::open(&PathBuf::from(lock_name))?;
    if !lock.try_lock()? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "event store {} already has an active writer",
                path.display()
            ),
        )
        .into());
    }
    Ok(lock)
}

fn patch_event_id(payload: &mut EventPayload, event_id: u64) -> Result<()> {
    if let EventPayload::Unknown(value) = payload {
        let object = value.as_object_mut().ok_or_else(|| {
            Error::InvalidArgument("event payload must be a JSON object".to_string())
        })?;
        object.insert("id".to_string(), event_id.into());
        return Ok(());
    }
    let EventPayload::ToolResult {
        structured: Some(structured),
        ..
    } = payload
    else {
        return Ok(());
    };
    if structured["kind"] == "file_content" {
        structured["read_event_id"] = event_id.into();
    }
    Ok(())
}

fn read_all(file: impl Read, path: &Path) -> Result<Vec<StoredEvent>> {
    let mut reader = EventReader::new(BufReader::new(file), path, 0);
    let mut events = Vec::new();
    while let Some(record) = reader.next()? {
        events.push(record.event);
    }
    Ok(events)
}

fn read_page_from(
    file: impl Read,
    path: &Path,
    offset: u64,
    after: u64,
    limit: usize,
) -> Result<Vec<StoredEvent>> {
    let mut reader = EventReader::new(BufReader::new(file), path, offset);
    let mut events = Vec::new();
    let mut page_bytes = 0;
    while let Some(record) = reader.next()? {
        if record.event.id <= after {
            continue;
        }
        page_bytes += record.bytes;
        events.push(record.event);
        if events.len() == limit.min(MAX_PAGE_RECORDS) || page_bytes >= MAX_PAGE_BYTES {
            break;
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests;
