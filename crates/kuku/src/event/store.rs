use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::types::{EventPayload, StoredEvent};

struct ReplayScan {
    events: Vec<StoredEvent>,
    last_valid_offset: u64,
    needs_truncation: bool,
}

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

        let scan = Self::scan(&path)?;
        if scan.needs_truncation {
            OpenOptions::new()
                .write(true)
                .open(&path)?
                .set_len(scan.last_valid_offset)?;
        }

        let next_id = scan.events.last().map_or(1, |event| event.id + 1);
        Ok(Self { path, next_id })
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id
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
        Ok(Self::scan(path.as_ref())?.events)
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
