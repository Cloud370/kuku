use std::io::{self, Read};

use memchr::{memchr, memchr_iter, memrchr};
use regex::{bytes::Regex as ByteRegex, Regex};
use regex_syntax::hir::{literal::Extractor, HirKind};

use crate::tool::builtin::common::{
    invalid_utf8, IoLimit, LimitedReader, ScanDeadline, Utf8Reader, FILE_BUFFER_BYTES,
};

pub(super) const SCAN_BYTES: IoLimit = IoLimit("search input byte budget exhausted");
const LINE_BYTES: IoLimit = IoLimit("a line exceeds the search buffer budget");

pub(super) struct FileSearch {
    regex: Regex,
    filter: Option<ByteRegex>,
    literal: bool,
    buffer: Vec<u8>,
    buffer_limit: usize,
    deadline: ScanDeadline,
}

impl FileSearch {
    pub(super) fn new(
        pattern: &str,
        line_bytes: usize,
        deadline: ScanDeadline,
    ) -> Result<Self, regex::Error> {
        let regex = Regex::new(pattern)?;
        let hir = regex_syntax::parse(pattern).ok();
        let literal = hir
            .as_ref()
            .is_some_and(|hir| matches!(hir.kind(), HirKind::Literal(_)));
        let filter = hir.as_ref().and_then(candidate_filter);
        Ok(Self {
            regex,
            filter,
            literal,
            buffer: vec![0; FILE_BUFFER_BYTES.min(line_bytes.saturating_add(1))],
            buffer_limit: line_bytes.saturating_add(1),
            deadline,
        })
    }

    pub(super) fn scan(
        &mut self,
        input: &mut impl Read,
        bytes_left: &mut u64,
        mut matched: impl FnMut(usize, &str) -> bool,
    ) -> io::Result<()> {
        let mut reader = Utf8Reader::new(
            LimitedReader::new(input, bytes_left, SCAN_BYTES).with_deadline(self.deadline),
        );
        let mut line_number = 1;
        let mut end = 0;
        loop {
            if end == self.buffer.len() {
                let capacity = end.saturating_mul(2).min(self.buffer_limit);
                if end == capacity {
                    return Err(io::Error::other(LINE_BYTES));
                }
                self.buffer
                    .try_reserve_exact(capacity - end)
                    .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
                self.buffer.resize(capacity, 0);
            }
            let count = match reader.read(&mut self.buffer[end..]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            let complete = if count == 0 {
                end
            } else {
                let Some(newline) = memrchr(b'\n', &self.buffer[end..end + count]) else {
                    end += count;
                    continue;
                };
                end + newline + 1
            };
            end += count;
            let lines = &self.buffer[..complete];
            if !self.scan_block(lines, &mut line_number, &mut matched)? || count == 0 {
                return Ok(());
            }
            self.buffer.copy_within(complete..end, 0);
            end -= complete;
        }
    }

    fn scan_block(
        &self,
        lines: &[u8],
        line_number: &mut usize,
        matched: &mut impl FnMut(usize, &str) -> bool,
    ) -> io::Result<bool> {
        if let Some(filter) = &self.filter {
            let mut position = 0;
            while let Some(hit) = filter.find_at(lines, position) {
                let preceding = &lines[position..hit.start()];
                let start =
                    memrchr(b'\n', preceding).map_or(position, |offset| position + offset + 1);
                *line_number += memchr_iter(b'\n', &lines[position..start]).count();
                let end = memchr(b'\n', &lines[hit.end()..])
                    .map_or(lines.len(), |offset| hit.end() + offset + 1);
                let line = line_text(&lines[start..end])?;
                let literal_in_line = self.literal && hit.end() <= start + line.len();
                if (literal_in_line || self.regex.is_match(line)) && !matched(*line_number, line) {
                    return Ok(false);
                }
                position = end;
                *line_number += 1;
            }
            *line_number += memchr_iter(b'\n', &lines[position..]).count();
        } else {
            for line in lines.split_inclusive(|&byte| byte == b'\n') {
                let line = line_text(line)?;
                if self.regex.is_match(line) && !matched(*line_number, line) {
                    return Ok(false);
                }
                *line_number += 1;
            }
        }
        Ok(true)
    }
}

fn candidate_filter(hir: &regex_syntax::hir::Hir) -> Option<ByteRegex> {
    let required = match hir.kind() {
        HirKind::Concat(parts) => parts.as_slice(),
        _ => &[],
    };
    let extractor = Extractor::new();
    let (_, alternatives) = std::iter::once(hir)
        .chain(required)
        .filter_map(|part| {
            let mut prefixes = extractor.extract(part);
            prefixes.optimize_for_prefix_by_preference();
            let minimum = prefixes.min_literal_len().filter(|&length| length > 0)?;
            let alternatives = prefixes
                .literals()?
                .iter()
                .map(|literal| {
                    let text = std::str::from_utf8(literal.as_bytes()).ok()?;
                    (!text.contains('\n')).then(|| regex::escape(text))
                })
                .collect::<Option<Vec<_>>>()?;
            Some((minimum, alternatives))
        })
        .max_by_key(|(minimum, _)| *minimum)?;
    ByteRegex::new(&alternatives.join("|")).ok()
}

fn line_text(source: &[u8]) -> io::Result<&str> {
    let source = source
        .strip_suffix(b"\r\n")
        .or_else(|| source.strip_suffix(b"\n"))
        .unwrap_or(source);
    std::str::from_utf8(source).map_err(|_| invalid_utf8())
}

#[cfg(test)]
mod tests {
    use super::{FileSearch, ScanDeadline};

    #[test]
    fn scan_matches_the_public_line_regex_semantics() {
        let body = "a\n\n\u{feff}a\r\na\rb\na\r\nZZZ中\r\narrêt\n中\r";
        for pattern in [
            "a",
            "a|中",
            "a\\r",
            "^a$",
            "a.$",
            "(?m)^a$",
            "(?m:^)(?-m:a$)",
            "\\A(?:b|a)\\z",
            "\\ba\\b",
            "\\B",
            "(?i)arrÊt",
            "(?:[^\\n]*a|\\n)",
            "a*",
            "^$",
        ] {
            let regex = regex::Regex::new(pattern).unwrap();
            let expected: Vec<_> = body
                .lines()
                .enumerate()
                .filter(|(_, row)| regex.is_match(row))
                .map(|(index, row)| (index + 1, row.to_owned()))
                .collect();
            let mut observed = Vec::new();
            FileSearch::new(pattern, 12, ScanDeadline::default())
                .unwrap()
                .scan(&mut body.as_bytes(), &mut 1024, |number, line| {
                    observed.push((number, line.to_owned()));
                    true
                })
                .unwrap();
            assert_eq!(expected, observed, "{pattern}");
        }
    }

    #[test]
    fn scanning_stops_when_a_line_exceeds_its_buffer() {
        let mut input = std::io::repeat(b'a');
        let mut bytes_left = 128;
        let error = FileSearch::new("a", 12, ScanDeadline::default())
            .unwrap()
            .scan(&mut input, &mut bytes_left, |_, _| {
                panic!("incomplete line")
            })
            .unwrap_err();
        assert_eq!("a line exceeds the search buffer budget", error.to_string());
        assert_eq!(115, bytes_left);
    }
}
