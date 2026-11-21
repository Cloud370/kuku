use std::path::Path;

use crate::execution::{CommandInvocation, CommandOutputRange, CommandOutputSummary, OutputStream};

use super::reports::rust_report;

mod rules;
use rules::{Rule, Selection};

const MAX_SAVED_LINES: usize = 256;
const MAX_SAVED_BYTES: usize = 64 * 1024;
const MAX_LINE_BYTES: usize = 4096;

pub(super) struct Collector {
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
        let CommandInvocation::Program { program, args } = invocation else {
            return None;
        };
        let program = Path::new(program).file_name()?.to_str()?;
        if program != "cargo"
            && !(cfg!(windows)
                && (program.eq_ignore_ascii_case("cargo")
                    || program.eq_ignore_ascii_case("cargo.exe")))
        {
            return None;
        }
        if args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--message-format"
                    | "--format"
                    | "--nocapture"
                    | "--no-capture"
                    | "--show-output"
                    | "-v"
                    | "-vv"
                    | "--verbose"
                    | "-Z"
                    | "--config"
            ) || arg.starts_with("--message-format=")
                || arg.starts_with("--format=")
        }) {
            return None;
        }
        let command = std::iter::once("cargo".to_owned())
            .chain(args.iter().map(|arg| {
                if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                    serde_json::Value::String(arg.clone()).to_string()
                } else {
                    arg.clone()
                }
            }))
            .collect::<Vec<_>>()
            .join(" ");
        Some(Self {
            rule: rules::select(&command)?,
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
        let test_failure = (trimmed.starts_with("test ") && trimmed.ends_with(" ... FAILED"))
            || trimmed.starts_with("---- ")
            || (trimmed.starts_with("thread ") && trimmed.contains("panicked at "));
        let report = rust_report(trimmed);
        let finished = trimmed.starts_with("Finished ") && trimmed.contains(" in ");
        self.finished |= finished || report.is_some();
        self.failed |= trimmed.starts_with("error:")
            || trimmed.starts_with("error[")
            || report.is_some_and(|report| report.failed > 0)
            || test_failure;
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
            || (exit_code == 0 && !self.finished)
            || (exit_code != 0 && !self.failed)
        {
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
