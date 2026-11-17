use std::io::{self, BufRead};
use std::path::Path;

use crate::error::{Error, Result};

use super::{StoredEvent, MAX_RECORD_BYTES};

pub(super) struct EventRecord {
    pub(super) event: StoredEvent,
    pub(super) offset: u64,
    pub(super) bytes: usize,
}

pub(super) struct EventReader<'a, R> {
    reader: R,
    path: &'a Path,
    buffer: Vec<u8>,
    offset: u64,
    committed_len: u64,
    previous_id: u64,
    incomplete_tail: bool,
}

impl<'a, R: BufRead> EventReader<'a, R> {
    pub(super) fn new(reader: R, path: &'a Path, offset: u64) -> Self {
        Self {
            reader,
            path,
            buffer: Vec::new(),
            offset,
            committed_len: offset,
            previous_id: 0,
            incomplete_tail: false,
        }
    }

    pub(super) fn next(&mut self) -> Result<Option<EventRecord>> {
        loop {
            let offset = self.offset;
            let line = read_bounded_line(&mut self.reader, &mut self.buffer).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "cannot read {} at byte {offset}: {error}",
                        self.path.display()
                    ),
                )
            })?;
            if line.bytes == 0 {
                return Ok(None);
            }
            self.offset += line.bytes;
            if !line.terminated {
                self.incomplete_tail = true;
                return Ok(None);
            }
            if line.oversized {
                return Err(
                    self.invalid_at(offset, &format!("record exceeds {MAX_RECORD_BYTES} bytes"))
                );
            }
            let bytes = self.buffer.strip_suffix(b"\n").unwrap();
            let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
            if bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
                self.committed_len = self.offset;
                continue;
            }
            let event = serde_json::from_slice::<StoredEvent>(bytes)
                .map_err(|error| self.invalid_at(offset, &error.to_string()))?;
            if event.id <= self.previous_id {
                return Err(self.invalid_at(
                    offset,
                    &format!(
                        "event id {} is not greater than previous id {}",
                        event.id, self.previous_id
                    ),
                ));
            }
            self.previous_id = event.id;
            self.committed_len = self.offset;
            return Ok(Some(EventRecord {
                event,
                offset,
                bytes: line.bytes as usize,
            }));
        }
    }

    pub(super) fn committed_len(&self) -> u64 {
        self.committed_len
    }

    pub(super) fn incomplete_tail(&self) -> bool {
        self.incomplete_tail
    }

    fn invalid_at(&self, offset: u64, message: &str) -> Error {
        Error::InvalidEventStream(format!(
            "{} at byte {offset}: {message}",
            self.path.display()
        ))
    }
}

struct LineRead {
    bytes: u64,
    terminated: bool,
    oversized: bool,
}

fn read_bounded_line(reader: &mut impl BufRead, buffer: &mut Vec<u8>) -> io::Result<LineRead> {
    buffer.clear();
    let mut bytes = 0;
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(LineRead {
                bytes,
                terminated: false,
                oversized,
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if consumed <= MAX_RECORD_BYTES - buffer.len() && !oversized {
            buffer.extend_from_slice(&available[..consumed]);
        } else {
            oversized = true;
        }
        bytes += consumed as u64;
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(LineRead {
                bytes,
                terminated: true,
                oversized,
            });
        }
    }
}
