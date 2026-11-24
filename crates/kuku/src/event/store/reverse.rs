use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::{Error, Result};

use super::reader::EventReader;
use super::{MAX_PAGE_BYTES, MAX_PAGE_RECORDS, MAX_RECORD_BYTES, StoredEvent};

const READ_CHUNK_BYTES: usize = 8192;

pub(super) fn find_page_end(
    file: &mut (impl Read + Seek),
    path: &Path,
    end: u64,
    before: u64,
) -> Result<u64> {
    let mut low = 0;
    let mut high = end;
    let mut boundary = end;
    while high.saturating_sub(low) > READ_CHUNK_BYTES as u64 {
        let midpoint = low + (high - low) / 2;
        file.seek(SeekFrom::Start(midpoint))?;
        let mut reader = BufReader::new(&mut *file);
        let offset = midpoint + reader.skip_until(b'\n')? as u64;
        if offset >= high {
            high = midpoint;
            continue;
        }
        match EventReader::new(reader, path, offset).next()? {
            Some(record) if record.event.id < before => low = record.offset + record.bytes as u64,
            Some(record) => {
                high = record.offset.min(high);
                boundary = record.offset.min(boundary);
            }
            None => high = midpoint,
        }
    }
    Ok(boundary)
}

pub(super) fn read_before(
    file: &mut (impl Read + Seek),
    path: &Path,
    end: u64,
    before: Option<u64>,
    limit: usize,
) -> Result<Vec<StoredEvent>> {
    if limit == 0 || before.is_some_and(|id| id <= 1) {
        return Ok(Vec::new());
    }
    let limit = limit.min(MAX_PAGE_RECORDS);
    let mut events = Vec::new();
    let mut bytes = 0;
    visit_before(file, path, end, before, |event, size| {
        bytes += size;
        if !events.is_empty() && bytes > MAX_PAGE_BYTES {
            return Ok(false);
        }
        events.push(event);
        Ok(events.len() < limit)
    })?;
    events.reverse();
    Ok(events)
}

pub(super) fn visit_before(
    file: &mut (impl Read + Seek),
    path: &Path,
    end: u64,
    before: Option<u64>,
    mut visit: impl FnMut(StoredEvent, usize) -> Result<bool>,
) -> Result<()> {
    if before.is_some_and(|id| id <= 1) {
        return Ok(());
    }
    let mut reader = ReverseReader {
        file,
        chunk: [0; READ_CHUNK_BYTES],
        remaining: 0,
        position: end,
    };
    let mut previous = None;
    let mut line = Vec::new();
    reader.line(None)?;
    while let Some(offset) = reader.line(Some(&mut line))? {
        if line.len() >= MAX_RECORD_BYTES {
            return Err(invalid_at(
                path,
                offset,
                &format!("record exceeds {MAX_RECORD_BYTES} bytes"),
            ));
        }
        line.reverse();
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let event: StoredEvent = serde_json::from_slice(&line)
            .map_err(|error| invalid_at(path, offset, &error.to_string()))?;
        if event.id == 0 || previous.is_some_and(|id| id <= event.id) {
            return Err(invalid_at(
                path,
                offset,
                &format!("event id {} is out of order", event.id),
            ));
        }
        previous = Some(event.id);
        if before.is_some_and(|id| event.id >= id) {
            continue;
        }
        if !visit(event, line.len() + 1)? {
            break;
        }
    }
    Ok(())
}

struct ReverseReader<'a, R> {
    file: &'a mut R,
    chunk: [u8; READ_CHUNK_BYTES],
    remaining: usize,
    position: u64,
}

impl<R: Read + Seek> ReverseReader<'_, R> {
    fn line(&mut self, mut output: Option<&mut Vec<u8>>) -> Result<Option<u64>> {
        if let Some(output) = &mut output {
            output.clear();
        }
        let mut consumed = false;
        loop {
            if self.remaining == 0 && !self.refill()? {
                return Ok(consumed.then_some(0));
            }
            let newline = self.chunk[..self.remaining]
                .iter()
                .rposition(|byte| *byte == b'\n');
            let start = newline.map_or(0, |index| index + 1);
            if let Some(output) = &mut output {
                let remaining = MAX_RECORD_BYTES - output.len();
                output.extend(
                    self.chunk[start..self.remaining]
                        .iter()
                        .rev()
                        .take(remaining),
                );
            }
            consumed = true;
            self.remaining = start.saturating_sub(1);
            if newline.is_some() {
                return Ok(Some(self.position + start as u64));
            }
        }
    }

    fn refill(&mut self) -> Result<bool> {
        if self.position == 0 {
            return Ok(false);
        }
        self.remaining = self.position.min(READ_CHUNK_BYTES as u64) as usize;
        self.position -= self.remaining as u64;
        self.file.seek(SeekFrom::Start(self.position))?;
        self.file.read_exact(&mut self.chunk[..self.remaining])?;
        Ok(true)
    }
}

fn invalid_at(path: &Path, offset: u64, message: &str) -> Error {
    Error::InvalidEventStream(format!("{} at byte {offset}: {message}", path.display()))
}
