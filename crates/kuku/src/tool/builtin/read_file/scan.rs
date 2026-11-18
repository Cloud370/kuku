use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{ReadBy, ReadRequest};
use crate::tool::builtin::common::{format_content_hash, StableFile};

const READ_BUFFER_BYTES: usize = 64 * 1024;

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
) -> io::Result<ReadView> {
    let mut file = StableFile::open(path)?;
    let mut buffer = [0; READ_BUFFER_BYTES];
    let mut carry = 0;
    let mut hash = Sha256::new();
    let mut view = ViewBuilder::new(request, read_by, max_bytes);
    loop {
        let read = match file.read(&mut buffer[carry..]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            break;
        }
        let end = carry + read;
        hash.update(&buffer[carry..end]);
        let valid = valid_prefix(&buffer[..end])?;
        view.push(&buffer[carry..end])?;
        buffer.copy_within(valid..end, 0);
        carry = end - valid;
    }
    if carry != 0 {
        return Err(invalid_utf8());
    }
    file.verify(path)?;
    view.finish(format_content_hash(hash))
}

fn valid_prefix(bytes: &[u8]) -> io::Result<usize> {
    match std::str::from_utf8(bytes) {
        Ok(_) => Ok(bytes.len()),
        Err(error) if error.error_len().is_none() => Ok(error.valid_up_to()),
        Err(_) => Err(invalid_utf8()),
    }
}

fn invalid_utf8() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "file is not valid UTF-8")
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
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            if self.selects_current_line() && !self.view.truncated {
                let buffer_limit = self.max_bytes.saturating_add(2);
                if self.line.len().saturating_add(part.len()) > buffer_limit {
                    self.view.truncated = true;
                    self.line.clear();
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
        }
        self.view.total_lines += 1;
        self.line_start = self.view.size_bytes;
        self.line.clear();
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
