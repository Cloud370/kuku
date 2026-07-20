use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

use crate::error::{Error, Result};

use super::types::{EventPayload, StoredEvent};

struct ReplayScan {
    events: Vec<StoredEvent>,
    last_valid_offset: u64,
    needs_truncation: bool,
}

type EventObserver = Arc<dyn Fn(&StoredEvent) + Send + Sync>;

#[derive(Default)]
struct TailState {
    initialized: bool,
    last_id: u64,
    modified: Option<SystemTime>,
    valid_offset: u64,
    #[cfg(test)]
    full_scan_count: u64,
}

#[derive(Default)]
struct SharedStoreState {
    observers: Mutex<Vec<EventObserver>>,
    publication: Mutex<()>,
    tail: Mutex<TailState>,
}

/// Append-only store for reading and writing events to a session's events.jsonl.
pub struct EventStore {
    path: PathBuf,
    shared: Arc<SharedStoreState>,
}

impl EventStore {
    /// Open an event store, creating parent directories and repairing truncated lines if needed.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let requested_path = path.as_ref();
        if let Some(parent) = requested_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let path = normalized_store_path(requested_path)?;
        let shared = shared_store_state(&path);

        let mut file_lock = event_file_lock(&path)?;
        file_lock.lock()?;
        let mut file = open_event_file(&path)?;
        let mut tail = mutex_lock(&shared.tail);
        Self::reconcile_tail(&mut file, &mut tail)?;
        drop(tail);

        Ok(Self { path, shared })
    }

    pub(crate) fn next_id(&self) -> u64 {
        mutex_lock(&self.shared.tail).last_id + 1
    }

    /// Register a callback for events durably written by [`Self::append_synced`].
    ///
    /// Registrations are shared by every store opened for the same normalized path.
    pub fn register_observer(&self, observer: Arc<dyn Fn(&StoredEvent) + Send + Sync>) {
        let _publication = mutex_lock(&self.shared.publication);
        mutex_lock(&self.shared.observers).push(observer);
    }

    /// Append a new event to the store and return the stored event with its assigned ID.
    pub fn append(&mut self, payload: EventPayload) -> Result<StoredEvent> {
        self.append_with_durability(payload, false)
    }

    /// Append one event and flush it durably before returning.
    pub fn append_synced(&mut self, payload: EventPayload) -> Result<StoredEvent> {
        self.append_with_durability(payload, true)
    }

    fn append_with_durability(&self, payload: EventPayload, durable: bool) -> Result<StoredEvent> {
        let _publication = mutex_lock(&self.shared.publication);
        let event = {
            let mut file_lock = event_file_lock(&self.path)?;
            file_lock.lock()?;
            let mut file = open_event_file(&self.path)?;
            let mut tail = mutex_lock(&self.shared.tail);
            Self::reconcile_tail(&mut file, &mut tail)?;

            let event_id = tail.last_id.checked_add(1).ok_or_else(|| {
                Error::InvalidEventStream("event id space is exhausted".to_owned())
            })?;
            let mut payload = payload;
            patch_tool_result_event_id(&mut payload, event_id);
            let event = StoredEvent {
                id: event_id,
                payload,
            };
            let mut encoded = serde_json::to_vec(&event)?;
            encoded.push(b'\n');

            file.seek(SeekFrom::Start(tail.valid_offset))?;
            file.set_len(tail.valid_offset)?;
            file.write_all(&encoded)?;
            file.flush()?;
            if durable {
                file.sync_data()?;
            }

            tail.last_id = event.id;
            tail.modified = file.metadata()?.modified().ok();
            tail.valid_offset = tail
                .valid_offset
                .checked_add(encoded.len() as u64)
                .ok_or_else(|| {
                    Error::InvalidEventStream("event stream offset is exhausted".to_owned())
                })?;
            event
        };

        if durable {
            let observers = mutex_lock(&self.shared.observers).clone();
            for observer in observers {
                observer(&event);
            }
        }
        Ok(event)
    }

    /// Read all events from an events.jsonl file.
    pub fn replay(path: impl AsRef<Path>) -> Result<Vec<StoredEvent>> {
        let path = normalized_store_path(path.as_ref())?;
        let mut file_lock = event_file_lock(&path)?;
        file_lock.lock()?;
        Ok(Self::scan(&path)?.events)
    }

    fn scan(path: &Path) -> Result<ReplayScan> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(ReplayScan {
                    events: Vec::new(),
                    last_valid_offset: 0,
                    needs_truncation: false,
                });
            }
            Err(error) => return Err(error.into()),
        };
        Self::scan_from(&mut file, 0, 0)
    }

    fn scan_from(file: &mut File, offset: u64, previous_id: u64) -> Result<ReplayScan> {
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut previous_id = previous_id;
        let mut current_offset = offset;
        let mut last_valid_offset = offset;
        let mut line_number = 0;
        let mut buffer = Vec::new();

        loop {
            buffer.clear();
            let bytes_read = reader.read_until(b'\n', &mut buffer)?;
            if bytes_read == 0 {
                break;
            }

            line_number += 1;
            current_offset += bytes_read as u64;

            let has_newline = buffer.ends_with(b"\n");
            let line = Self::trim_line_ending(&buffer);

            if !has_newline {
                return Ok(ReplayScan {
                    events,
                    last_valid_offset,
                    needs_truncation: true,
                });
            }

            if Self::is_blank_line(line) {
                last_valid_offset = current_offset;
                continue;
            }

            let event = serde_json::from_slice::<StoredEvent>(line).map_err(|error| {
                Error::InvalidEventStream(format!("invalid event at line {line_number}: {error}"))
            })?;

            if event.id <= previous_id {
                return Err(Error::InvalidEventStream(format!(
                    "event id {} at line {} is not greater than previous id {}",
                    event.id, line_number, previous_id
                )));
            }

            previous_id = event.id;
            events.push(event);
            last_valid_offset = current_offset;
        }

        Ok(ReplayScan {
            events,
            last_valid_offset,
            needs_truncation: false,
        })
    }

    fn reconcile_tail(file: &mut File, tail: &mut TailState) -> Result<()> {
        let metadata = file.metadata()?;
        let file_len = metadata.len();
        let modified = metadata.modified().ok();
        if !tail.initialized
            || file_len < tail.valid_offset
            || (file_len == tail.valid_offset && modified != tail.modified)
        {
            #[cfg(test)]
            {
                tail.full_scan_count += 1;
            }
            let scan = Self::scan_from(file, 0, 0)?;
            Self::apply_scan(file, tail, scan, 0)?;
        } else if file_len > tail.valid_offset {
            let previous_id = tail.last_id;
            let scan = Self::scan_from(file, tail.valid_offset, previous_id)?;
            Self::apply_scan(file, tail, scan, previous_id)?;
        }
        tail.initialized = true;
        tail.modified = file.metadata()?.modified().ok();
        Ok(())
    }

    fn apply_scan(
        file: &mut File,
        tail: &mut TailState,
        scan: ReplayScan,
        previous_id: u64,
    ) -> Result<()> {
        if scan.needs_truncation {
            file.set_len(scan.last_valid_offset)?;
        }
        tail.last_id = scan.events.last().map_or(previous_id, |event| event.id);
        tail.valid_offset = scan.last_valid_offset;
        Ok(())
    }

    #[cfg(test)]
    fn full_scan_count_for_test(&self) -> u64 {
        mutex_lock(&self.shared.tail).full_scan_count
    }

    fn trim_line_ending(line: &[u8]) -> &[u8] {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        line.strip_suffix(b"\r").unwrap_or(line)
    }

    fn is_blank_line(line: &[u8]) -> bool {
        line.iter().all(|byte| byte.is_ascii_whitespace())
    }
}

fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn normalized_store_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(std::fs::canonicalize(path)?);
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let parent = std::fs::canonicalize(parent)?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(ErrorKind::InvalidInput, "event store path has no file name")
    })?;
    Ok(parent.join(file_name))
}

fn open_event_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}

fn event_file_lock(path: &Path) -> Result<fslock::LockFile> {
    let lock_path = path.with_extension(format!(
        "{}lock",
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!("{extension}."))
            .unwrap_or_default()
    ));
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(fslock::LockFile::open(&lock_path)?)
}

fn patch_tool_result_event_id(payload: &mut EventPayload, event_id: u64) {
    let EventPayload::ToolResult {
        structured: Some(structured),
        ..
    } = payload
    else {
        return;
    };

    if structured["kind"] == "file_content" {
        structured["read_event_id"] = serde_json::Value::from(event_id);
    }
}

fn shared_store_state(path: &Path) -> Arc<SharedStoreState> {
    static STORES: OnceLock<Mutex<HashMap<PathBuf, Arc<SharedStoreState>>>> = OnceLock::new();

    let stores = STORES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut stores = mutex_lock(stores);
    stores
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(SharedStoreState::default()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventPayload;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn turn_started(turn: u64) -> EventPayload {
        EventPayload::TurnStarted {
            ts: format!("2026-07-20T00:00:{turn:02}Z"),
            conversation: "conversation-1".to_owned(),
            turn,
        }
    }

    #[test]
    fn unknown_event_type_is_preserved_not_failed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let content = concat!(
            "{\"id\":1,\"ts\":\"a\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s\",\"created_at\":\"a\",\"kuku_version\":\"0\"}\n",
            "{\"id\":2,\"ts\":\"b\",\"kind\":\"future.event\",\"turn\":1,\"custom\":\"x\"}\n",
        );
        std::fs::write(&path, content).unwrap();
        let events = EventStore::replay(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1].payload, EventPayload::Unknown(_)));
    }

    #[test]
    fn concurrent_handles_assign_monotonic_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|worker| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = EventStore::open(path).unwrap();
                    barrier.wait();
                    for turn in 1..=32 {
                        store
                            .append_synced(turn_started(worker * 32 + turn))
                            .unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }

        let ids = EventStore::replay(path)
            .unwrap()
            .into_iter()
            .map(|event| event.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, (1..=64).collect::<Vec<_>>());
    }

    #[test]
    fn append_synced_repairs_a_truncated_tail_before_appending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        store.append_synced(turn_started(1)).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(br#"{"id":2,"kind":"turn.started""#)
            .unwrap();

        let appended = store.append_synced(turn_started(2)).unwrap();

        assert_eq!(appended.id, 2);
        assert_eq!(EventStore::replay(path).unwrap().len(), 2);
    }

    #[test]
    fn observers_run_in_append_order_after_the_event_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let observed = Arc::new(Mutex::new(Vec::new()));
        let durable = Arc::new(AtomicBool::new(false));
        let registration = EventStore::open(&path).unwrap();
        registration.register_observer({
            let path = path.clone();
            let observed = Arc::clone(&observed);
            let durable = Arc::clone(&durable);
            Arc::new(move |event| {
                let replayed = EventStore::replay(&path).unwrap();
                durable.store(replayed.last() == Some(event), Ordering::SeqCst);
                observed.lock().unwrap().push(event.id);
            })
        });
        drop(registration);
        let mut reopened = EventStore::open(&path).unwrap();

        reopened.append_synced(turn_started(1)).unwrap();
        reopened.append_synced(turn_started(2)).unwrap();

        assert!(durable.load(Ordering::SeqCst));
        assert_eq!(*observed.lock().unwrap(), vec![1, 2]);
    }

    #[test]
    fn repeated_appends_and_reopens_do_not_rescan_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        let scans_after_open = store.full_scan_count_for_test();

        for turn in 1..=64 {
            store.append_synced(turn_started(turn)).unwrap();
        }
        for _ in 0..16 {
            let reopened = EventStore::open(&path).unwrap();
            assert_eq!(reopened.full_scan_count_for_test(), scans_after_open);
        }

        assert_eq!(store.full_scan_count_for_test(), scans_after_open);
    }
}
