use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{ReadBy, ReadRequest};
use crate::tool::builtin::common::{
    FILE_BUFFER_BYTES, FILE_SCAN_BYTES, FileCancellation, LimitedReader, StableFile, Utf8Reader,
    format_content_hash, invalid_utf8, valid_utf8_prefix,
};

const LINE_SAMPLE_BYTES: usize = 256;

pub(super) struct ReadView {
    pub(super) content_hash: String,
    pub(super) raw_text: String,
    pub(super) model_content: String,
    pub(super) start_byte: usize,
    pub(super) size_bytes: usize,
    pub(super) line_count: usize,
    pub(super) total_lines: usize,
    pub(super) truncated: bool,
}

pub(super) fn scan(
    path: &Path,
    request: &ReadRequest<'_>,
    read_by: ReadBy,
    max_bytes: usize,
    mut scan_bytes: u64,
    cancelled: &FileCancellation<'_>,
) -> io::Result<ReadView> {
    let mut file = StableFile::open(path, cancelled)?;
    file.check_size(scan_bytes, FILE_SCAN_BYTES)?;
    let mut reader = Utf8Reader::new(LimitedReader::new(
        &mut file,
        &mut scan_bytes,
        FILE_SCAN_BYTES,
    ));
    let mut buffer = [0; FILE_BUFFER_BYTES];
    let mut hash = Sha256::new();
    let mut view = ViewBuilder::new(request, read_by, max_bytes);
    loop {
        let read = match reader.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        view.push(&buffer[..read])?;
    }
    file.verify(path)?;
    view.finish(format_content_hash(hash))
}

struct ViewBuilder {
    read_by: ReadBy,
    offset: usize,
    limit: usize,
    max_bytes: usize,
    line_start: usize,
    line: Vec<u8>,
    view: ReadView,
}

impl ViewBuilder {
    fn new(request: &ReadRequest<'_>, read_by: ReadBy, max_bytes: usize) -> Self {
        Self {
            read_by,
            offset: request.offset,
            limit: request.limit.unwrap_or(usize::MAX),
            max_bytes,
            line_start: 0,
            line: Vec::new(),
            view: ReadView {
                content_hash: String::new(),
                raw_text: String::new(),
                model_content: String::new(),
                start_byte: 0,
                size_bytes: 0,
                line_count: 0,
                total_lines: 0,
                truncated: false,
            },
        }
    }

    fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut start = 0;
        for end in memchr::memchr_iter(b'\n', bytes)
            .map(|newline| newline + 1)
            .chain(std::iter::once(bytes.len()))
        {
            if start == end {
                break;
            }
            let part = &bytes[start..end];
            start = end;
            if self.selects_current_line() && !self.view.truncated {
                let buffer_limit = self.max_bytes.saturating_add(2);
                if self.line.len().saturating_add(part.len()) > buffer_limit {
                    self.view.truncated = true;
                    if matches!(self.read_by, ReadBy::Model)
                        && self.view.total_lines + 1 == self.offset
                    {
                        let sample_bytes = self.max_bytes.min(LINE_SAMPLE_BYTES);
                        self.line.truncate(sample_bytes);
                        let remaining = sample_bytes - self.line.len();
                        self.line
                            .extend_from_slice(&part[..part.len().min(remaining)]);
                    }
                } else {
                    self.line.extend_from_slice(part);
                }
            }
            self.view.size_bytes += part.len();
            if part.ends_with(b"\n") {
                self.finish_line()?;
            }
        }
        Ok(())
    }

    fn selects_current_line(&self) -> bool {
        (self.view.total_lines + 1)
            .checked_sub(self.offset)
            .is_some_and(|index| index < self.limit)
    }

    fn finish_line(&mut self) -> io::Result<()> {
        if self.selects_current_line() {
            if self.view.total_lines + 1 == self.offset {
                self.view.start_byte = self.line_start;
            }
            if !self.view.truncated {
                self.append_line()?;
            }
            if self.view.truncated
                && self.view.total_lines + 1 == self.offset
                && matches!(self.read_by, ReadBy::Model)
            {
                self.sample_line()?;
            }
        }
        self.view.total_lines += 1;
        self.line_start = self.view.size_bytes;
        self.line.clear();
        Ok(())
    }

    fn sample_line(&mut self) -> io::Result<()> {
        use std::fmt::Write;

        let number = self.offset.to_string();
        let max_prefix = (self.max_bytes - number.len() - 1).min(LINE_SAMPLE_BYTES);
        let bytes = &self.line[..self.line.len().min(max_prefix)];
        let prefix = &bytes[..valid_utf8_prefix(bytes)?];
        let line = std::str::from_utf8(prefix).map_err(|_| invalid_utf8())?;
        let _ = write!(self.view.model_content, "{number}\t{line}");
        Ok(())
    }

    fn append_line(&mut self) -> io::Result<()> {
        let raw = std::str::from_utf8(&self.line).map_err(|_| invalid_utf8())?;
        let line = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(raw);
        match self.read_by {
            ReadBy::Executor
                if self.view.raw_text.len().saturating_add(raw.len()) > self.max_bytes =>
            {
                self.view.truncated = true;
                return Ok(());
            }
            ReadBy::Executor => {}
            ReadBy::Model => {
                let number = (self.view.total_lines + 1).to_string();
                let content = &mut self.view.model_content;
                let separator = usize::from(!content.is_empty());
                if content.len() + separator + number.len() + 1 + line.len() > self.max_bytes {
                    self.view.truncated = true;
                    return Ok(());
                }
                if separator == 1 {
                    content.push('\n');
                }
                content.push_str(&number);
                content.push('\t');
                content.push_str(line);
            }
        }
        self.view.raw_text.push_str(raw);
        self.view.line_count += 1;
        Ok(())
    }

    fn finish(mut self, hash: String) -> io::Result<ReadView> {
        if self.line_start < self.view.size_bytes {
            self.finish_line()?;
        }
        if self.offset > self.view.total_lines {
            self.view.start_byte = self.view.size_bytes;
        }
        self.view.content_hash = hash;
        Ok(self.view)
    }
}
