use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::{Error, Result};

use super::types::{EventPayload, StoredEvent};

struct ReplayScan {
    events: Vec<StoredEvent>,
    last_valid_offset: u64,
    needs_truncation: bool,
}

/// Append-only store for reading and writing events to a session's events.jsonl.
pub struct EventStore {
    path: PathBuf,
    next_id: Arc<Mutex<u64>>,
}

impl EventStore {
    /// Open an event store, creating parent directories and repairing truncated lines if needed.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file_lock = event_file_lock(&path)?;
        file_lock.lock()?;
        let scan = Self::scan(&path)?;
        if scan.needs_truncation {
            OpenOptions::new()
                .write(true)
                .open(&path)?
                .set_len(scan.last_valid_offset)?;
        }

        let next_id = scan.events.last().map_or(1, |event| event.id + 1);
        Ok(Self {
            next_id: next_id_counter(&path, next_id),
            path,
        })
    }

    pub(crate) fn next_id(&self) -> u64 {
        *self.next_id.lock().unwrap()
    }

    /// Append a new event to the store and return the stored event with its assigned ID.
    pub fn append(&mut self, payload: EventPayload) -> Result<StoredEvent> {
        let mut file_lock = event_file_lock(&self.path)?;
        file_lock.lock()?;

        let mut next_id = self.next_id.lock().unwrap();
        let scan = Self::scan(&self.path)?;
        if scan.needs_truncation {
            OpenOptions::new()
                .write(true)
                .open(&self.path)?
                .set_len(scan.last_valid_offset)?;
        }

        let event_id = scan.events.last().map_or(1, |event| event.id + 1);
        let mut payload = payload;
        patch_tool_result_event_id(&mut payload, event_id);
        let event = StoredEvent {
            id: event_id,
            payload,
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, &event)?;
        file.write_all(b"\n")?;
        file.flush()?;
        *next_id = event.id + 1;
        Ok(event)
    }

    /// Read all events from an events.jsonl file.
    pub fn replay(path: impl AsRef<Path>) -> Result<Vec<StoredEvent>> {
        let path = path.as_ref();
        let mut file_lock = event_file_lock(path)?;
        file_lock.lock()?;
        Ok(Self::scan(path)?.events)
    }

    fn scan(path: &Path) -> Result<ReplayScan> {
        let file = match File::open(path) {
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
        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut previous_id = 0;
        let mut current_offset = 0_u64;
        let mut last_valid_offset = 0_u64;
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

    fn trim_line_ending(line: &[u8]) -> &[u8] {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        line.strip_suffix(b"\r").unwrap_or(line)
    }

    fn is_blank_line(line: &[u8]) -> bool {
        line.iter().all(|byte| byte.is_ascii_whitespace())
    }
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

fn next_id_counter(path: &Path, initial_next_id: u64) -> Arc<Mutex<u64>> {
    static NEXT_IDS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<u64>>>>> = OnceLock::new();

    let counters = NEXT_IDS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut counters = counters.lock().unwrap();
    let counter = counters
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(initial_next_id)))
        .clone();
    let mut next_id = counter.lock().unwrap();
    if *next_id < initial_next_id {
        *next_id = initial_next_id;
    }
    drop(next_id);
    counter
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventPayload;

    #[test]
    fn unknown_event_type_is_preserved_not_failed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let content = concat!(
            "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"a\",\"schema_version\":1,\"session_id\":\"s\",\"created_at\":\"a\",\"kuku_version\":\"0\"}\n",
            "{\"id\":2,\"type\":\"future.event\",\"ts\":\"b\",\"turn\":1,\"custom\":\"x\"}\n",
        );
        std::fs::write(&path, content).unwrap();
        let events = EventStore::replay(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1].payload, EventPayload::Unknown(_)));
    }
}
