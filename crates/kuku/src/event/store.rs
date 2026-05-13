use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::types::{EventPayload, StoredEvent};

pub struct EventStore {
    path: PathBuf,
    next_id: u64,
}

impl EventStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let events = Self::replay(&path)?;
        let next_id = events.last().map_or(1, |event| event.id + 1);
        Ok(Self { path, next_id })
    }

    pub fn append(&mut self, payload: EventPayload) -> Result<StoredEvent> {
        let event = StoredEvent {
            id: self.next_id,
            payload,
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, &event)?;
        file.write_all(b"\n")?;
        file.flush()?;
        self.next_id += 1;
        Ok(event)
    }

    pub fn replay(path: impl AsRef<Path>) -> Result<Vec<StoredEvent>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut previous_id = 0;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let event = match serde_json::from_str::<StoredEvent>(&line) {
                Ok(event) => event,
                Err(_) => break,
            };

            if event.id <= previous_id {
                return Err(Error::InvalidEventStream(format!(
                    "event id {} is not greater than previous id {}",
                    event.id, previous_id
                )));
            }

            previous_id = event.id;
            events.push(event);
        }

        Ok(events)
    }
}
