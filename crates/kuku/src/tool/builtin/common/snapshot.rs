use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, StoredEvent};
use crate::tool::{ToolErrorReason, ToolResultEnvelope};

pub(crate) struct ReadSnapshot {
    pub(crate) event_id: u64,
    start_line: usize,
    line_count: usize,
    is_full_file_snapshot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExecutorRead {
    pub(crate) canonical_path: PathBuf,
    pub(crate) content_hash: String,
    pub(crate) range: Range<usize>,
    pub(crate) size_bytes: usize,
    pub(crate) complete: bool,
}

pub(crate) struct WriteSnapshot {
    pub(crate) event_id: u64,
    pub(crate) content_hash: String,
    coverage: ReadCoverage,
}

impl WriteSnapshot {
    pub(crate) fn restrict_matches(
        &self,
        content: &str,
        text: &str,
        mut offsets: Vec<usize>,
    ) -> Vec<usize> {
        if let ReadCoverage::Ranges(spans) = &self.coverage {
            let ranges = merged_ranges(spans.iter().filter_map(|span| span.bytes(content)));
            offsets.retain(|offset| {
                let index = ranges.partition_point(|range| range.end <= *offset);
                ranges.get(index).is_some_and(|range| {
                    *offset >= range.start && *offset + text.len() <= range.end
                })
            });
        }
        offsets
    }
}

enum ReadCoverage {
    Full,
    Ranges(Vec<ObservedSpan>),
}

enum ObservedSpan {
    Bytes(Range<usize>),
    Lines { start: usize, count: usize },
}

impl ObservedSpan {
    fn bytes(&self, content: &str) -> Option<Range<usize>> {
        match self {
            Self::Bytes(range) => Some(range.clone()),
            Self::Lines { start, count } => {
                let mut lines = content.split_inclusive('\n');
                let prefix: usize = lines
                    .by_ref()
                    .take(start.checked_sub(1)?)
                    .map(str::len)
                    .sum();
                let len: usize = lines.take(*count).map(str::len).sum();
                Some(prefix..prefix + len)
            }
        }
    }
}

pub(crate) enum WriteEvidence<'a> {
    Model {
        conversation: &'a ConversationAddress,
        events: &'a [StoredEvent],
    },
    Executor(&'a [ExecutorRead]),
}

impl WriteEvidence<'_> {
    pub(crate) fn find(
        &self,
        path: &Path,
        require_full: bool,
        required_text: Option<&str>,
        current_hash: &str,
    ) -> WriteSnapshotLookup {
        match self {
            Self::Model {
                conversation,
                events,
            } => find_write_snapshot(
                events,
                conversation,
                path,
                require_full,
                required_text,
                Some(current_hash),
            ),
            Self::Executor(reads) => {
                find_executor_snapshot(reads, path, require_full, current_hash)
            }
        }
    }

    pub(crate) fn is_model(&self) -> bool {
        matches!(self, Self::Model { .. })
    }
}

pub(crate) enum WriteSnapshotLookup {
    Found(WriteSnapshot),
    Rejected(ToolErrorReason),
}

pub(crate) fn expected_version(args: &Value) -> Result<Option<&str>, ToolResultEnvelope> {
    let Some(value) = args.get("expected_version") else {
        return Ok(None);
    };
    value.as_str().map(Some).ok_or_else(|| {
        ToolResultEnvelope::error(
            "failed: expected_version must be a string",
            "expected_version must be a read_file content_hash",
        )
    })
}

pub(crate) fn require_version(
    relative: &str,
    expected: Option<&str>,
    current: Option<&str>,
) -> Result<(), ToolResultEnvelope> {
    if let Some(expected) = expected {
        let matches = current.is_some_and(|current| {
            current == expected
                || current.strip_prefix("sha256:").is_some_and(|digest| {
                    digest.len() == 64
                        && digest.eq_ignore_ascii_case(
                            expected.strip_prefix("sha256:").unwrap_or(expected),
                        )
                })
        });
        if !matches {
            return Err(snapshot_stale_error(relative, 0));
        }
    }
    Ok(())
}

pub(crate) fn snapshot_stale_error(relative: &str, event_id: u64) -> ToolResultEnvelope {
    let summary = if event_id == 0 {
        format!("failed: {relative} changed since it was read")
    } else {
        format!("failed: {relative} changed since event {event_id}")
    };
    ToolResultEnvelope::error_with_reason(
        summary,
        format!("file changed since it was read; read {relative} again before modifying"),
        ToolErrorReason::SnapshotStale,
    )
}

pub(crate) fn find_covering_read(
    events: &[StoredEvent],
    conversation: &crate::conversation::address::ConversationAddress,
    canonical_path: &Path,
    content_hash: &str,
    start_line: usize,
    line_count: usize,
) -> Option<ReadSnapshot> {
    let canonical_path = canonical_path.to_string_lossy();
    crate::context::replay::effective_snapshot_events(events, conversation)
        .into_iter()
        .rev()
        .find_map(|event| {
            let EventPayload::ToolResult {
                status,
                structured: Some(structured),
                ..
            } = &event.payload
            else {
                return None;
            };
            if status != "ok"
                || structured["kind"] != "file_content"
                || structured["cached"] == true
                || structured["read_by"] == "executor"
            {
                return None;
            }
            if structured["canonical_path"].as_str()? != canonical_path
                || structured["content_hash"].as_str()? != content_hash
            {
                return None;
            }
            let snapshot = ReadSnapshot {
                event_id: structured["read_event_id"].as_u64().unwrap_or(event.id),
                start_line: structured["start_line"].as_u64()? as usize,
                line_count: structured["line_count"].as_u64()? as usize,
                is_full_file_snapshot: structured["is_full_file_snapshot"]
                    .as_bool()
                    .unwrap_or(false),
            };
            if snapshot.covers(start_line, line_count) {
                Some(snapshot)
            } else {
                None
            }
        })
}

impl ReadSnapshot {
    fn covers(&self, start_line: usize, line_count: usize) -> bool {
        self.is_full_file_snapshot
            || (self.start_line == start_line && self.line_count == line_count)
    }
}

pub(crate) fn find_write_snapshot(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
    canonical_path: &Path,
    require_full_file: bool,
    required_text: Option<&str>,
    current_content_hash: Option<&str>,
) -> WriteSnapshotLookup {
    let canonical_path = canonical_path.to_string_lossy();
    let mut saw_path_snapshot = false;
    let mut stale_snapshot = None;
    let mut current_snapshot: Option<WriteSnapshot> = None;
    for event in crate::context::replay::effective_snapshot_events(events, conversation)
        .into_iter()
        .rev()
    {
        let EventPayload::ToolResult {
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            continue;
        };
        if structured["read_by"] == "executor"
            || structured["canonical_path"].as_str() != Some(canonical_path.as_ref())
        {
            continue;
        }
        let Some(content_hash) = structured["content_hash"].as_str() else {
            continue;
        };
        saw_path_snapshot = true;
        let Some(coverage) = model_coverage(structured, require_full_file, required_text) else {
            continue;
        };
        let event_id = structured["read_event_id"].as_u64().unwrap_or(event.id);
        let snapshot = WriteSnapshot {
            event_id,
            content_hash: content_hash.to_owned(),
            coverage,
        };
        if current_content_hash.is_some_and(|current| current != content_hash) {
            stale_snapshot.get_or_insert(snapshot);
            continue;
        }
        if current_content_hash.is_none() || matches!(snapshot.coverage, ReadCoverage::Full) {
            return WriteSnapshotLookup::Found(snapshot);
        }
        let Some(current) = &mut current_snapshot else {
            current_snapshot = Some(snapshot);
            continue;
        };
        if let (ReadCoverage::Ranges(current), ReadCoverage::Ranges(new)) =
            (&mut current.coverage, snapshot.coverage)
        {
            current.extend(new);
        }
    }
    if let Some(snapshot) = current_snapshot.or(stale_snapshot) {
        return WriteSnapshotLookup::Found(snapshot);
    }
    let reason = if require_full_file {
        ToolErrorReason::FullSnapshotRequired
    } else if saw_path_snapshot && required_text.is_some() {
        ToolErrorReason::OldTextNotVisible
    } else {
        ToolErrorReason::SnapshotRequired
    };
    WriteSnapshotLookup::Rejected(reason)
}

fn model_coverage(
    structured: &Value,
    require_full: bool,
    required_text: Option<&str>,
) -> Option<ReadCoverage> {
    if structured["is_full_file_snapshot"] == true {
        return Some(ReadCoverage::Full);
    }
    if require_full {
        return None;
    }
    let raw = visible_snapshot_raw_text(structured)?;
    if required_text.is_some_and(|text| text_match_offsets(raw, text).is_empty()) {
        return None;
    }
    let span = match structured["start_byte"].as_u64() {
        Some(start) => {
            let start = usize::try_from(start).ok()?;
            ObservedSpan::Bytes(start..start.checked_add(raw.len())?)
        }
        None => ObservedSpan::Lines {
            start: usize::try_from(structured["start_line"].as_u64()?).ok()?,
            count: usize::try_from(structured["line_count"].as_u64()?).ok()?,
        },
    };
    Some(ReadCoverage::Ranges(vec![span]))
}

fn find_executor_snapshot(
    reads: &[ExecutorRead],
    canonical_path: &Path,
    require_full: bool,
    current_hash: &str,
) -> WriteSnapshotLookup {
    let mut reads = reads
        .iter()
        .filter(|read| read.canonical_path == canonical_path);
    let Some(latest) = reads.next_back() else {
        return WriteSnapshotLookup::Rejected(ToolErrorReason::SnapshotRequired);
    };
    let mut current = std::iter::once(latest)
        .chain(reads)
        .filter(|read| read.content_hash == current_hash);
    let Some(head) = current.next() else {
        return WriteSnapshotLookup::Found(WriteSnapshot {
            event_id: 0,
            content_hash: latest.content_hash.clone(),
            coverage: ReadCoverage::Ranges(Vec::new()),
        });
    };
    let mut full = head.complete;
    let ranges = merged_ranges(std::iter::once(head).chain(current).map(|read| {
        full |= read.complete;
        read.range.clone()
    }));
    full |= ranges.first().is_some_and(|range| {
        range.start == 0 && range.end == head.size_bytes && head.size_bytes != 0
    });
    if require_full && !full {
        return WriteSnapshotLookup::Rejected(ToolErrorReason::FullSnapshotRequired);
    }
    WriteSnapshotLookup::Found(WriteSnapshot {
        event_id: 0,
        content_hash: current_hash.to_owned(),
        coverage: if full {
            ReadCoverage::Full
        } else {
            ReadCoverage::Ranges(ranges.into_iter().map(ObservedSpan::Bytes).collect())
        },
    })
}

fn merged_ranges(ranges: impl Iterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let mut ranges = ranges.filter(|range| !range.is_empty()).collect::<Vec<_>>();
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for next in ranges {
        match merged.last_mut() {
            Some(previous) if previous.end >= next.start => {
                previous.end = previous.end.max(next.end)
            }
            _ => merged.push(next),
        }
    }
    merged
}

pub(crate) fn text_match_offsets(content: &str, text: &str) -> Vec<usize> {
    let content_bytes = content.as_bytes();
    let text_bytes = text.as_bytes();
    content
        .match_indices(text)
        .map(|(offset, _)| offset)
        .filter(|offset| {
            text_bytes.iter().enumerate().all(|(text_offset, byte)| {
                if *byte != b'\n' || (text_offset > 0 && text_bytes[text_offset - 1] == b'\r') {
                    return true;
                }
                let content_offset = offset + text_offset;
                content_offset == 0 || content_bytes[content_offset - 1] != b'\r'
            })
        })
        .collect()
}

fn visible_snapshot_raw_text(structured: &Value) -> Option<&str> {
    let raw_text = structured["raw_text"].as_str()?;
    let line_count = structured["line_count"].as_u64()? as usize;
    let visible_len = raw_text
        .split_inclusive('\n')
        .take(line_count)
        .map(str::len)
        .sum();
    raw_text.get(..visible_len)
}
