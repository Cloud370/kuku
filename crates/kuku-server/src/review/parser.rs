//! Pure Git output parsers and canonical record helpers.

use std::collections::BTreeMap;

use crate::api::{ApiError, ChangeKind, DiffHunk, DiffLine, DiffLineKind};

use super::{invalid_git_output, payload_too_large};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedChange {
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
    pub(crate) kind: ChangeKind,
    pub(crate) staged: bool,
    pub(crate) worktree: bool,
    pub(crate) identity: Vec<u8>,
}

type Numstat = BTreeMap<String, (Option<u32>, Option<u32>)>;

pub(crate) fn parse_porcelain_v2(bytes: &[u8]) -> Result<Vec<ParsedChange>, ApiError> {
    let records = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.is_empty() {
            index += 1;
            continue;
        }
        let text = std::str::from_utf8(record).map_err(|_| invalid_git_output())?;
        let (change, consumes_old_path) = parse_status_record(text)?;
        let mut change = change;
        if consumes_old_path {
            index += 1;
            let old = records.get(index).ok_or_else(invalid_git_output)?;
            change.old_path = Some(
                std::str::from_utf8(old)
                    .map_err(|_| invalid_git_output())?
                    .to_owned(),
            );
        }
        changes.push(change);
        index += 1;
    }
    Ok(changes)
}

fn parse_status_record(text: &str) -> Result<(ParsedChange, bool), ApiError> {
    if let Some(path) = text.strip_prefix("? ") {
        return Ok((
            ParsedChange {
                path: path.to_owned(),
                old_path: None,
                kind: ChangeKind::Untracked,
                staged: false,
                worktree: true,
                identity: b"untracked".to_vec(),
            },
            false,
        ));
    }
    let record_kind = text
        .as_bytes()
        .first()
        .copied()
        .ok_or_else(invalid_git_output)?;
    let field_count = match record_kind {
        b'1' => 9,
        b'2' => 10,
        b'u' => 11,
        _ => return Err(invalid_git_output()),
    };
    let fields = text.splitn(field_count, ' ').collect::<Vec<_>>();
    if fields.len() != field_count || fields[1].len() != 2 {
        return Err(invalid_git_output());
    }
    let xy = fields[1].as_bytes();
    let kind = if record_kind == b'u' || matches!(xy, [b'U', _] | [_, b'U']) {
        ChangeKind::Conflicted
    } else if record_kind == b'2' {
        match fields[8].as_bytes().first() {
            Some(b'R') => ChangeKind::Renamed,
            Some(b'C') => ChangeKind::Copied,
            _ => return Err(invalid_git_output()),
        }
    } else if xy.contains(&b'A') {
        ChangeKind::Added
    } else if xy.contains(&b'D') {
        ChangeKind::Deleted
    } else if xy.contains(&b'T') {
        ChangeKind::TypeChanged
    } else {
        ChangeKind::Modified
    };
    Ok((
        ParsedChange {
            path: fields[field_count - 1].to_owned(),
            old_path: None,
            kind,
            staged: xy[0] != b'.',
            worktree: xy[1] != b'.',
            identity: fields[..field_count - 1].join(" ").into_bytes(),
        },
        record_kind == b'2',
    ))
}

pub(crate) fn parse_numstat(bytes: &[u8]) -> Result<Numstat, ApiError> {
    let records = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut stats = BTreeMap::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.is_empty() {
            index += 1;
            continue;
        }
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let additions = parse_count(fields.next().ok_or_else(invalid_git_output)?)?;
        let deletions = parse_count(fields.next().ok_or_else(invalid_git_output)?)?;
        let path = fields.next().ok_or_else(invalid_git_output)?;
        let current = if path.is_empty() {
            index += 2;
            records.get(index).ok_or_else(invalid_git_output)?
        } else {
            path
        };
        let current = std::str::from_utf8(current)
            .map_err(|_| invalid_git_output())?
            .to_owned();
        stats.insert(current, (additions, deletions));
        index += 1;
    }
    Ok(stats)
}

fn parse_count(bytes: &[u8]) -> Result<Option<u32>, ApiError> {
    if bytes == b"-" {
        return Ok(None);
    }
    let value = std::str::from_utf8(bytes).map_err(|_| invalid_git_output())?;
    value
        .parse::<u32>()
        .map(Some)
        .map_err(|_| invalid_git_output())
}

#[derive(Debug, Default)]
pub(super) struct UnifiedParser {
    pub(super) binary: bool,
    pub(super) old_line: u32,
    pub(super) new_line: u32,
    pub(super) in_hunk: bool,
    pub(super) lines: Vec<DiffLine>,
}

impl UnifiedParser {
    pub(super) fn push_line(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let text = std::str::from_utf8(bytes).map_err(|_| invalid_git_output())?;
        if text.starts_with("GIT binary patch") || text.starts_with("Binary files ") {
            self.binary = true;
            return Ok(());
        }
        if let Some(header) = text.strip_prefix("@@ -") {
            let (old, new) = parse_hunk_header(header)?;
            self.old_line = old;
            self.new_line = new;
            self.in_hunk = true;
            return Ok(());
        }
        if !self.in_hunk {
            return Ok(());
        }
        let (kind, old_line, new_line, content) = if let Some(content) = text.strip_prefix(' ') {
            let old = self.old_line;
            let new = self.new_line;
            self.old_line = self.old_line.saturating_add(1);
            self.new_line = self.new_line.saturating_add(1);
            (DiffLineKind::Context, Some(old), Some(new), content)
        } else if let Some(content) = text.strip_prefix('-') {
            let old = self.old_line;
            self.old_line = self.old_line.saturating_add(1);
            (DiffLineKind::Deletion, Some(old), None, content)
        } else if let Some(content) = text.strip_prefix('+') {
            let new = self.new_line;
            self.new_line = self.new_line.saturating_add(1);
            (DiffLineKind::Addition, None, Some(new), content)
        } else if let Some(content) = text.strip_prefix("\\ ") {
            (DiffLineKind::NoNewlineMarker, None, None, content)
        } else {
            return Ok(());
        };
        self.lines.push(DiffLine {
            kind,
            old_line,
            new_line,
            text: content.to_owned(),
        });
        Ok(())
    }
}

fn parse_hunk_header(header: &str) -> Result<(u32, u32), ApiError> {
    let (old, rest) = header.split_once(" +").ok_or_else(invalid_git_output)?;
    let new = rest.split_once(" @@").map_or(rest, |(range, _)| range);
    Ok((parse_range_start(old)?, parse_range_start(new)?))
}

fn parse_range_start(range: &str) -> Result<u32, ApiError> {
    range
        .split(',')
        .next()
        .ok_or_else(invalid_git_output)?
        .parse::<u32>()
        .map_err(|_| invalid_git_output())
}

pub(super) fn lines_to_hunks(lines: Vec<DiffLine>) -> Vec<DiffHunk> {
    if lines.is_empty() {
        return Vec::new();
    }
    let old_start = lines.iter().find_map(|line| line.old_line).unwrap_or(0);
    let new_start = lines.iter().find_map(|line| line.new_line).unwrap_or(0);
    let old_lines = lines.iter().filter(|line| line.old_line.is_some()).count() as u32;
    let new_lines = lines.iter().filter(|line| line.new_line.is_some()).count() as u32;
    vec![DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines,
    }]
}

pub(super) fn logical_index_records(bytes: &[u8]) -> Result<Vec<Vec<u8>>, ApiError> {
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            record
                .rsplit(|byte| *byte == b'\n')
                .find(|record| {
                    record.len() > 8
                        && record[..6].iter().all(|byte| matches!(byte, b'0'..=b'7'))
                        && record[6] == b' '
                })
                .map(ToOwned::to_owned)
        })
        .collect())
}

pub(super) fn index_by_path(records: &[Vec<u8>]) -> Result<BTreeMap<String, Vec<u8>>, ApiError> {
    let mut result = BTreeMap::new();
    for record in records {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(invalid_git_output)?;
        let path = std::str::from_utf8(&record[tab + 1..])
            .map_err(|_| invalid_git_output())?
            .to_owned();
        result
            .entry(path)
            .and_modify(|prior: &mut Vec<u8>| {
                prior.push(0);
                prior.extend_from_slice(record);
            })
            .or_insert_with(|| record.clone());
    }
    Ok(result)
}

pub(super) fn nul_strings(bytes: &[u8]) -> Result<Vec<String>, ApiError> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            std::str::from_utf8(record)
                .map(str::to_owned)
                .map_err(|_| invalid_git_output())
        })
        .collect()
}

pub(super) fn logical_line_count(bytes: &[u8]) -> Result<u32, ApiError> {
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
    let lines = newlines + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    u32::try_from(lines).map_err(|_| payload_too_large())
}
