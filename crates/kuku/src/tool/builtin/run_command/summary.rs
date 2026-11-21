use crate::execution::{CommandInvocation, CommandOutputRange, CommandOutputSummary, OutputStream};

mod formats;
mod rules;
mod typescript;
use formats::{Format, Observation};
use rules::{Rule, Selection};

const MAX_SAVED_LINES: usize = 256;
const MAX_SAVED_BYTES: usize = 64 * 1024;
const MAX_LINE_BYTES: usize = 4096;

pub(super) struct Collector {
    format: Format,
    rule: &'static Rule,
    stdout: LineBuffer,
    stderr: LineBuffer,
    lines: Vec<EvidenceLine>,
    saved_bytes: usize,
    finished: bool,
    failed: bool,
    unrecognized: bool,
}

#[derive(Default)]
struct LineBuffer {
    partial: Vec<u8>,
    offset: Option<u64>,
}

struct EvidenceLine {
    text: String,
    source: CommandOutputRange,
    blank: bool,
}

pub(super) struct Summary {
    pub(super) text: String,
    pub(super) source: CommandOutputSummary,
}

impl Collector {
    pub(super) fn new(invocation: &CommandInvocation) -> Option<Self> {
        let (format, rule) = Format::select(invocation)?;
        Some(Self {
            format,
            rule,
            stdout: LineBuffer::default(),
            stderr: LineBuffer::default(),
            lines: Vec::new(),
            saved_bytes: 0,
            finished: false,
            failed: false,
            unrecognized: false,
        })
    }

    pub(super) fn observe(&mut self, stream: OutputStream, mut offset: u64, bytes: &[u8]) {
        if self.unrecognized {
            return;
        }
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            let line = match stream {
                OutputStream::Stdout => &mut self.stdout,
                OutputStream::Stderr => &mut self.stderr,
            };
            if line.partial.len() + part.len() > MAX_LINE_BYTES {
                self.unrecognized = true;
                return;
            }
            line.offset.get_or_insert(offset);
            offset += part.len() as u64;
            line.partial.extend_from_slice(part);
            if part.last() != Some(&b'\n') {
                continue;
            }
            let start = line.offset.take().expect("output line has a start");
            let mut buffer = std::mem::take(&mut line.partial);
            self.line(stream, start, offset, &buffer);
            buffer.clear();
            let line = match stream {
                OutputStream::Stdout => &mut self.stdout,
                OutputStream::Stderr => &mut self.stderr,
            };
            line.partial = buffer;
            if self.unrecognized {
                return;
            }
        }
    }

    fn line(&mut self, stream: OutputStream, offset: u64, end_offset: u64, bytes: &[u8]) {
        let Ok(raw) = std::str::from_utf8(bytes) else {
            self.unrecognized = true;
            return;
        };
        let line = self.rule.prepare_line(raw.trim_end_matches(['\n', '\r']));
        if line.chars().any(|ch| ch.is_control() && ch != '\t') {
            self.unrecognized = true;
            return;
        }
        let trimmed = line.trim();
        if trimmed.starts_with('{') && serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            self.unrecognized = true;
            return;
        }
        match self.format.observe(stream, trimmed) {
            Some(Observation::Finished) => self.finished = true,
            Some(Observation::Failed) => self.failed = true,
            None => {}
        }
        if !self.rule.keeps(&line) {
            return;
        }
        if self.saved_bytes + line.len() > MAX_SAVED_BYTES || self.lines.len() >= MAX_SAVED_LINES {
            self.unrecognized = true;
            return;
        }
        self.saved_bytes += line.len();
        let blank = trimmed.is_empty();
        self.lines.push(EvidenceLine {
            text: line.into_owned(),
            source: CommandOutputRange {
                stream,
                offset,
                end_offset,
            },
            blank,
        });
    }

    pub(super) fn summarize(self, exit_code: i32) -> Option<Summary> {
        if self.unrecognized
            || self.stdout.offset.is_some()
            || self.stderr.offset.is_some()
            || self.lines.is_empty()
        {
            return None;
        }
        if self.format == Format::TypeScript {
            return typescript::summarize(self.rule, &self.lines, exit_code);
        }
        if (exit_code == 0 && !self.finished) || (exit_code != 0 && !self.failed) {
            return None;
        }
        let selected = self.rule.select_lines(self.lines.len());
        if exit_code != 0
            && selected
                .iter()
                .any(|entry| !matches!(entry, Selection::Line(_)))
        {
            return None;
        }
        let mut selected_nonblank = 0;
        let mut ranges: Vec<CommandOutputRange> = Vec::new();
        let mut text = Vec::with_capacity(selected.len());
        for entry in selected {
            match entry {
                Selection::Line(index) => {
                    let line = &self.lines[index];
                    selected_nonblank += usize::from(!line.blank);
                    text.push(line.text.clone());
                    if let Some(last) = ranges.last_mut().filter(|last| {
                        last.stream == line.source.stream && last.end_offset == line.source.offset
                    }) {
                        last.end_offset = line.source.end_offset;
                    } else {
                        ranges.push(line.source.clone());
                    }
                }
                Selection::Omitted(count) => text.push(format!("... ({count} lines omitted)")),
                Selection::Truncated(count) => text.push(format!("... ({count} lines truncated)")),
            }
        }
        if selected_nonblank != self.lines.iter().filter(|line| !line.blank).count() {
            return None;
        }
        Some(Summary {
            text: text.join("\n"),
            source: CommandOutputSummary::Rule {
                rule: self.rule.name.clone(),
                ranges,
            },
        })
    }
}

#[cfg(test)]
mod tests;
