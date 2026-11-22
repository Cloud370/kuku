//! Durable append-only session event storage.
//! Open log paths must remain stable during a writer's lifetime.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::error::{Error, Result};
use crate::util::fs_sync;

use super::types::{EventPayload, StoredEvent};

mod archive;
mod codec;
mod file_history;
mod index;
mod packing;
mod reader;
mod recovery;
mod reverse;

use archive::Layout;
use codec::serialize_record;
use index::JournalIndex;
pub(crate) use index::{ContextMarks, HistoryMarks, JournalSummary, RouteSelectors, TurnOwners};
pub use index::{MessageRewind, UserMessagePreview};
use reader::EventReader;
pub(crate) use recovery::Replay;
use recovery::{Checkpoint, ReplayVisitor};

const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_PAGE_BYTES: usize = 1024 * 1024;
const MAX_PAGE_RECORDS: usize = 256;
const INDEX_STRIDE: usize = 128;
const SCAN_BUFFER_BYTES: usize = 128 * 1024;

type StoreRegistry = Mutex<HashMap<PathBuf, StoreEntry>>;

#[derive(Clone, Copy, Serialize, Deserialize)]
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
        drop(writer.journal.take());
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
    journal: Option<LockedJournal>,
    layout: Layout,
    file_lock: Option<fslock::LockFile>,
    index: Vec<EventOffset>,
    event_count: usize,
    turn_count: u64,
    facts: JournalIndex,
    committed_len: u64,
    last_id: u64,
    poison: Option<String>,
    #[cfg(test)]
    fail_next_write: bool,
}

struct LockedJournal {
    file: File,
}

impl LockedJournal {
    fn writer(file: File, path: &Path) -> Result<Self> {
        fs_sync::lock_writer(&file, path)?;
        Ok(Self { file })
    }

    fn reader(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        fs_sync::lock_reader(&file, path)?;
        Ok(Self { file })
    }
}

impl Drop for LockedJournal {
    fn drop(&mut self) {
        // NOTE: A duplicated or inherited handle can outlive this owner.
        let _ = self.file.unlock();
    }
}

impl Writer {
    fn open(path: &Path, visit: &mut Option<&mut ReplayVisitor<'_>>) -> Result<Self> {
        let file_lock = acquire_file_lock(path)?;
        archive::clear_staging(path)?;
        let file = fs_sync::write_options()
            .read(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let journal = LockedJournal::writer(file, path)?;
        let layout = Layout::read(&journal.file)?;
        let end = layout.logical_len(journal.file.metadata()?.len());
        let checkpoint = recovery::load(&layout, &journal.file, path);
        let (mut index, mut facts, mut event_count, mut turn_count, mut last_id, start) =
            match checkpoint {
                Some(checkpoint) if recovery::offer(&checkpoint, visit)? => (
                    checkpoint.index,
                    checkpoint.facts,
                    checkpoint.event_count,
                    layout.prefix_turns,
                    layout.prefix_head,
                    layout.prefix_bytes,
                ),
                _ => (Vec::new(), JournalIndex::default(), 0, 0, 0, 0),
            };
        let mut reader = EventReader::new(
            BufReader::with_capacity(SCAN_BUFFER_BYTES, layout.reader(&journal.file, end, start)),
            path,
            start,
        );
        while let Some(record) = reader.next()? {
            if record.event.id <= last_id {
                return Err(Error::InvalidEventStream(format!(
                    "{}: event {} follows event {last_id}",
                    path.display(),
                    record.event.id
                )));
            }
            if event_count.is_multiple_of(INDEX_STRIDE) {
                index.push(EventOffset {
                    id: record.event.id,
                    offset: record.offset,
                });
            }
            event_count += 1;
            turn_count += u64::from(matches!(
                record.event.payload,
                EventPayload::TurnStarted { .. }
            ));
            last_id = record.event.id;
            facts.observe_location(&record.event, record.offset, record.bytes);
            if let Some(visit) = visit.as_mut() {
                visit(Replay::Event(record.event))?;
            }
        }
        let committed_len = reader.committed_len();
        let incomplete_tail = reader.incomplete_tail();
        drop(reader);
        if incomplete_tail {
            journal.file.set_len(layout.tail_offset(committed_len)?)?;
            journal.file.sync_data()?;
        }
        journal.file.sync_all()?;
        fs_sync::sync_parent(path)?;
        Ok(Self {
            journal: Some(journal),
            layout,
            file_lock: Some(file_lock),
            index,
            event_count,
            turn_count,
            facts,
            committed_len,
            last_id,
            poison: None,
            #[cfg(test)]
            fail_next_write: false,
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

    fn write(&mut self, path: &Path, event_id: u64, records: &[u8]) -> Result<()> {
        #[cfg(test)]
        let injected_failure = if self.fail_next_write {
            self.fail_next_write = false;
            Some(io::Error::other("injected event journal write failure"))
        } else {
            None
        };
        #[cfg(not(test))]
        let injected_failure: Option<io::Error> = None;
        let file = &mut self.journal.as_mut().unwrap().file;
        let result = injected_failure.map_or_else(
            || {
                file.seek(SeekFrom::Start(
                    self.layout.tail_offset(self.committed_len)?,
                ))
                .and_then(|_| file.write_all(records))
                .and_then(|()| fs_sync::sync_file(file))
            },
            Err,
        );
        if let Err(error) = result {
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
        Ok(())
    }

    fn advance(&mut self, event: &StoredEvent, bytes: usize) {
        if self.event_count.is_multiple_of(INDEX_STRIDE) {
            self.index.push(EventOffset {
                id: event.id,
                offset: self.committed_len,
            });
        }
        self.facts
            .observe_location(event, self.committed_len, bytes);
        self.event_count += 1;
        self.turn_count += u64::from(matches!(event.payload, EventPayload::TurnStarted { .. }));
        self.committed_len += bytes as u64;
        self.last_id = event.id;
    }

    fn replay(&mut self, path: &Path) -> Result<Vec<StoredEvent>> {
        self.ensure_healthy(path)?;
        let file = &self.journal.as_ref().unwrap().file;
        read_all(self.layout.reader(file, self.committed_len, 0), path)
    }

    #[cfg(feature = "test_support")]
    fn committed_bytes(&mut self, path: &Path) -> Result<Vec<u8>> {
        self.ensure_healthy(path)?;
        let mut file = &self.journal.as_ref().unwrap().file;
        let position = file.stream_position()?;
        let result = (|| {
            let mut reader = self.layout.reader(file, self.committed_len, 0);
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            if bytes.len() as u64 != self.committed_len {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "committed journal prefix was truncated",
                )
                .into());
            }
            Ok(bytes)
        })();
        file.seek(SeekFrom::Start(position))?;
        result
    }

    fn offset_at(&self, after: u64) -> u64 {
        let checkpoint = self
            .index
            .partition_point(|entry| entry.id <= after)
            .saturating_sub(1);
        self.index.get(checkpoint).map_or(0, |entry| entry.offset)
    }

    fn read_after(&mut self, path: &Path, after: u64, limit: usize) -> Result<Vec<StoredEvent>> {
        self.ensure_healthy(path)?;
        if limit == 0 || after >= self.last_id {
            return Ok(Vec::new());
        }
        let mut start = self.offset_at(after);
        if after >= self.layout.prefix_head {
            start = start.max(self.layout.prefix_bytes);
        }
        let file = &self.journal.as_ref().unwrap().file;
        read_page_from(
            self.layout.reader(file, self.committed_len, start),
            path,
            start,
            after,
            limit,
        )
    }

    fn offset_before(&self, before: Option<u64>) -> u64 {
        before
            .and_then(|id| {
                self.index
                    .get(self.index.partition_point(|entry| entry.id < id))
            })
            .map_or(self.committed_len, |entry| entry.offset)
    }

    fn read_before(
        &mut self,
        path: &Path,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        self.ensure_healthy(path)?;
        let end = self.offset_before(before);
        reverse::read_before(
            &mut self
                .layout
                .reader(&self.journal.as_ref().unwrap().file, end, 0),
            path,
            end,
            before,
            limit,
        )
    }

    fn visit_after(
        &mut self,
        path: &Path,
        after: u64,
        visit: impl FnMut(StoredEvent) -> Result<()>,
    ) -> Result<u64> {
        let mut visit = visit;
        self.scan_after(path, after, |event| visit(event).map(|()| true))
    }

    fn scan_after(
        &self,
        path: &Path,
        after: u64,
        visit: impl FnMut(StoredEvent) -> Result<bool>,
    ) -> Result<u64> {
        self.ensure_healthy(path)?;
        if after >= self.last_id {
            return Ok(after);
        }
        let mut start = self.offset_at(after);
        if after >= self.layout.prefix_head {
            start = start.max(self.layout.prefix_bytes);
        }
        let file = &self.journal.as_ref().unwrap().file;
        scan_events(
            self.layout.reader(file, self.committed_len, start),
            path,
            start,
            after,
            visit,
        )
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
        Self::open_inner(path.as_ref(), None)
    }

    pub(crate) fn open_replaying(
        path: &Path,
        mut visit: impl FnMut(Replay<'_>) -> Result<bool>,
    ) -> Result<Self> {
        Self::open_inner(path, Some(&mut visit))
    }

    fn open_inner(path: &Path, mut visit: Option<&mut ReplayVisitor<'_>>) -> Result<Self> {
        let absolute = std::env::current_dir()?.join(path);
        let mut stores = registry().lock().unwrap();
        if let Ok(key) = fs::canonicalize(&absolute) {
            if let Some(inner) = find_active_store(&stores, &key) {
                drop(stores);
                if let Some(visit) = visit {
                    inner.writer.lock().unwrap().visit_recovery(&key, visit)?;
                }
                return Ok(Self { inner });
            }
        }
        if let Some(parent) = absolute.parent() {
            fs_sync::create_dir_all(parent)?;
        }
        let key = canonical_store_path(&absolute)?;
        if let Some(inner) = find_active_store(&stores, &key) {
            drop(stores);
            if let Some(visit) = visit {
                inner.writer.lock().unwrap().visit_recovery(&key, visit)?;
            }
            return Ok(Self { inner });
        }
        let path = key;
        let writer = Writer::open(&path, &mut visit)?;
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
        let id = writer.last_id.checked_add(1).ok_or_else(ids_exhausted)?;
        patch_event_id(&mut payload, id)?;
        let event = StoredEvent { id, payload };
        let record = serialize_record(&event)?;
        writer.write(&self.inner.path, id, &record)?;
        writer.advance(&event, record.len());
        drop(writer);
        self.changed().notify_waiters();
        Ok(event)
    }

    pub(crate) fn append_batch(
        &mut self,
        build: impl FnOnce(u64) -> Result<Vec<EventPayload>>,
    ) -> Result<Vec<StoredEvent>> {
        let path = &self.inner.path;
        let mut writer = self.inner.writer.lock().unwrap();
        writer.ensure_healthy(path)?;
        let first = writer.last_id.checked_add(1).ok_or_else(ids_exhausted)?;
        let payloads = build(first)?;
        if payloads.is_empty() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        let mut events = Vec::with_capacity(payloads.len());
        for (index, mut payload) in payloads.into_iter().enumerate() {
            let id = first.checked_add(index as u64).ok_or_else(ids_exhausted)?;
            patch_event_id(&mut payload, id)?;
            let event = StoredEvent { id, payload };
            let record = serialize_record(&event)?;
            let length = record.len();
            records.extend(record);
            events.push((event, length));
        }
        writer.write(path, first, &records)?;
        let events = events
            .into_iter()
            .map(|(event, bytes)| {
                writer.advance(&event, bytes);
                event
            })
            .collect();
        drop(writer);
        self.changed().notify_waiters();
        Ok(events)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_write_for_test(&self) {
        self.inner.writer.lock().unwrap().fail_next_write = true;
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
        let journal = LockedJournal::reader(&path)?;
        let layout = Layout::read(&journal.file)?;
        let end = layout.logical_len(journal.file.metadata()?.len());
        let start = recovery::page_start(&layout, &journal.file, &path, after);
        read_page_from(
            layout.reader(&journal.file, end, start),
            &path,
            start,
            after,
            limit,
        )
    }

    /// Read a bounded chronological page before a cursor, or the latest page when omitted.
    pub fn read_page_before(
        path: impl AsRef<Path>,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(path) = existing_path(path.as_ref())? else {
            return Ok(Vec::new());
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            return inner
                .writer
                .lock()
                .unwrap()
                .read_before(&path, before, limit);
        }
        let journal = LockedJournal::reader(&path)?;
        drop(stores);
        let layout = Layout::read(&journal.file)?;
        let mut end = layout.logical_len(journal.file.metadata()?.len());
        let mut reader = layout.reader(&journal.file, end, 0);
        if let Some(before) =
            before.filter(|id| end != layout.prefix_bytes || *id <= layout.prefix_head)
        {
            end = reverse::find_page_end(&mut reader, &path, end, before)?;
        }
        reverse::read_before(&mut reader, &path, end, before, limit)
    }

    /// Visit committed events newest first, stopping when the callback returns false.
    pub fn visit_committed_before(
        path: &Path,
        before: Option<u64>,
        mut visit: impl FnMut(StoredEvent) -> Result<bool>,
    ) -> Result<()> {
        Self::visit_committed_before_with_size(path, before, |event, _| visit(event))
    }

    pub(crate) fn visit_committed_before_with_size(
        path: &Path,
        before: Option<u64>,
        visit: impl FnMut(StoredEvent, usize) -> Result<bool>,
    ) -> Result<()> {
        let Some(path) = existing_path(path)? else {
            return Ok(());
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            let writer = inner.writer.lock().unwrap();
            writer.ensure_healthy(&path)?;
            let end = writer.offset_before(before);
            let file = &writer.journal.as_ref().unwrap().file;
            let mut reader = writer.layout.reader(file, writer.committed_len, end);
            return reverse::visit_before(&mut reader, &path, end, before, visit);
        }
        let journal = LockedJournal::reader(&path)?;
        drop(stores);
        let layout = Layout::read(&journal.file)?;
        let end = layout.logical_len(journal.file.metadata()?.len());
        let mut reader = layout.reader(&journal.file, end, 0);
        let end = before
            .filter(|id| end != layout.prefix_bytes || *id <= layout.prefix_head)
            .map_or(Ok(end), |id| {
                reverse::find_page_end(&mut reader, &path, end, id)
            })?;
        reverse::visit_before(&mut reader, &path, end, before, visit)
    }

    /// Read the last committed event ID without opening or repairing the writer.
    pub fn read_head(path: impl AsRef<Path>) -> Result<u64> {
        Ok(Self::read_page_before(path, None, 1)?
            .first()
            .map_or(0, |event| event.id))
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
        let journal = LockedJournal::reader(&path)?;
        let layout = Layout::read(&journal.file)?;
        let end = layout.logical_len(journal.file.metadata()?.len());
        read_all(layout.reader(&journal.file, end, 0), &path)
    }

    #[cfg(feature = "test_support")]
    pub(crate) fn read_active_journal_for_test(path: &Path) -> Result<Vec<u8>> {
        let path = fs::canonicalize(path)?;
        let stores = registry().lock().unwrap();
        let inner = find_active_store(&stores, &path).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "event store {} has no active writer",
                path.display()
            ))
        })?;
        drop(stores);
        let mut writer = inner.writer.lock().unwrap();
        writer.committed_bytes(&path)
    }

    /// Visit committed events after an exclusive cursor, stopping when the callback returns false.
    pub fn visit_committed(
        path: &Path,
        after: u64,
        visit: impl FnMut(StoredEvent) -> Result<bool>,
    ) -> Result<u64> {
        let Some(path) = existing_path(path)? else {
            return Ok(after);
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            return inner.writer.lock().unwrap().scan_after(&path, after, visit);
        }
        let journal = LockedJournal::reader(&path)?;
        let layout = Layout::read(&journal.file)?;
        let end = layout.logical_len(journal.file.metadata()?.len());
        let start = recovery::page_start(&layout, &journal.file, &path, after);
        drop(stores);
        scan_events(
            layout.reader(&journal.file, end, start),
            &path,
            start,
            after,
            visit,
        )
    }

    pub(crate) fn visit_after_path(
        path: &Path,
        after: u64,
        mut visit: impl FnMut(StoredEvent) -> Result<()>,
    ) -> Result<u64> {
        Self::visit_committed(path, after, |event| visit(event).map(|()| true))
    }

    pub(crate) fn journal_summary(path: &Path) -> Result<JournalSummary> {
        let Some(path) = existing_path(path)? else {
            return Ok(JournalSummary::default());
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            return Ok(inner.writer.lock().unwrap().facts.summary.clone());
        }
        let file = File::open(&path)?;
        drop(stores);
        let layout = Layout::read(&file)?;
        let end = layout.logical_len(file.metadata()?.len());
        let (mut summary, start) = recovery::load(&layout, &file, &path)
            .map(|checkpoint| (checkpoint.facts.summary, layout.prefix_bytes))
            .unwrap_or_default();
        let mut reader = EventReader::new(
            BufReader::with_capacity(SCAN_BUFFER_BYTES, layout.reader(&file, end, start)),
            &path,
            start,
        );
        loop {
            match reader.next() {
                Ok(Some(record)) => summary.observe(&record.event),
                Ok(None) => return Ok(summary),
                Err(Error::InvalidEventStream(_)) => {}
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) fn context_marks(path: &Path, address: &str) -> Result<ContextMarks> {
        Self::with_index(path, |facts, _| facts.context_marks(address))
    }

    pub(crate) fn history_marks(path: &Path, include_owners: bool) -> Result<HistoryMarks> {
        Self::with_index(path, |facts, cursor| {
            Ok(facts.history_marks(include_owners, cursor))
        })
    }

    fn with_index<T>(
        path: &Path,
        select: impl FnOnce(&JournalIndex, u64) -> Result<T>,
    ) -> Result<T> {
        let Some(path) = existing_path(path)? else {
            return select(&JournalIndex::default(), 0);
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            let writer = inner.writer.lock().unwrap();
            writer.ensure_healthy(&path)?;
            return select(&writer.facts, writer.last_id);
        }
        let journal = LockedJournal::reader(&path)?;
        drop(stores);
        let (facts, cursor) = read_index(&path, &journal.file)?;
        select(&facts, cursor)
    }

    pub(crate) fn rewind_facts(
        &self,
        address: &str,
        target: u64,
    ) -> Result<(u64, Vec<StoredEvent>)> {
        let writer = self.inner.writer.lock().unwrap();
        writer.ensure_healthy(&self.inner.path)?;
        Ok((
            writer.facts.rewind_start(address, target),
            writer.facts.rewind_controls(),
        ))
    }

    pub(crate) fn turn_terminal(&self, address: &str, turn: u64) -> Result<(bool, Option<u64>)> {
        let writer = self.inner.writer.lock().unwrap();
        writer.ensure_healthy(&self.inner.path)?;
        Ok(writer.facts.turn_terminal(address, turn))
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

fn existing_path(path: &Path) -> Result<Option<PathBuf>> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
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

fn ids_exhausted() -> Error {
    Error::InvalidEventStream("event IDs exhausted".into())
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

fn read_index(path: &Path, file: &File) -> Result<(JournalIndex, u64)> {
    let layout = Layout::read(file)?;
    let end = layout.logical_len(file.metadata()?.len());
    let (mut facts, start, after) = recovery::load(&layout, file, path)
        .map(|checkpoint| (checkpoint.facts, layout.prefix_bytes, layout.prefix_head))
        .unwrap_or_default();
    let mut cursor = after;
    let mut reader = EventReader::new(BufReader::new(layout.reader(file, end, start)), path, start);
    while let Some(record) = reader.next()? {
        if record.event.id > after {
            cursor = record.event.id;
            facts.observe_location(&record.event, record.offset, record.bytes);
        }
    }
    Ok((facts, cursor))
}

fn read_all(file: impl Read, path: &Path) -> Result<Vec<StoredEvent>> {
    let mut reader = EventReader::new(BufReader::new(file), path, 0);
    let mut events = Vec::new();
    while let Some(record) = reader.next()? {
        events.push(record.event);
    }
    Ok(events)
}

fn visit_events(
    file: impl Read,
    path: &Path,
    offset: u64,
    after: u64,
    mut visit: impl FnMut(StoredEvent) -> Result<()>,
) -> Result<u64> {
    scan_events(file, path, offset, after, |event| {
        visit(event).map(|()| true)
    })
}

fn scan_events(
    file: impl Read,
    path: &Path,
    offset: u64,
    after: u64,
    mut visit: impl FnMut(StoredEvent) -> Result<bool>,
) -> Result<u64> {
    let mut reader = EventReader::new(
        BufReader::with_capacity(SCAN_BUFFER_BYTES, file),
        path,
        offset,
    );
    let mut head = after;
    while let Some(record) = reader.next()? {
        if record.event.id > after {
            head = record.event.id;
            if !visit(record.event)? {
                break;
            }
        }
    }
    Ok(head)
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
