use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{
    EventOffset, JournalIndex, JournalSummary, Layout, StoredEvent, archive::MAX_CHECKPOINT,
    reverse,
};
use crate::Result;

#[expect(
    clippy::large_enum_variant,
    reason = "replay moves each record without a heap allocation"
)]
pub(crate) enum Replay<'a> {
    Checkpoint(&'a RawValue),
    Event(StoredEvent),
}

pub(super) type ReplayVisitor<'a> = dyn FnMut(Replay<'_>) -> Result<bool> + 'a;

#[derive(Deserialize, Serialize)]
pub(super) struct Checkpoint<I = Vec<EventOffset>, F = JournalIndex, R = Box<RawValue>> {
    pub(super) head: u64,
    pub(super) bytes: u64,
    pub(super) event_count: usize,
    pub(super) index: I,
    pub(super) facts: F,
    pub(super) runtime: R,
}

#[derive(Deserialize)]
struct SummaryFacts {
    summary: JournalSummary,
}

impl Checkpoint {
    pub(super) fn offset_at(&self, after: u64) -> Option<u64> {
        let count = self.index.partition_point(|entry| entry.id <= after);
        count.checked_sub(1).map(|index| self.index[index].offset)
    }
}

pub(super) fn page_start(layout: &Layout, file: &File, path: &Path, after: u64) -> u64 {
    if after >= layout.prefix_head {
        layout.prefix_bytes
    } else if after == 0 {
        0
    } else {
        load(layout, file, path)
            .and_then(|checkpoint| checkpoint.offset_at(after))
            .unwrap_or_default()
    }
}

pub(super) fn offer(
    checkpoint: &Checkpoint,
    visit: &mut Option<&mut ReplayVisitor<'_>>,
) -> Result<bool> {
    match visit.as_mut() {
        Some(visit) => visit(Replay::Checkpoint(&checkpoint.runtime)),
        None => Ok(true),
    }
}

pub(super) fn load(layout: &Layout, file: &File, path: &Path) -> Option<Checkpoint> {
    let encoded = layout.checkpoint(file).ok()??;
    let checkpoint: Checkpoint = serde_json::from_slice(&encoded).ok()?;
    if checkpoint.index.len() != checkpoint.event_count.div_ceil(super::INDEX_STRIDE)
        || checkpoint
            .index
            .last()
            .is_none_or(|entry| entry.id > checkpoint.head || entry.offset >= checkpoint.bytes)
    {
        return None;
    }
    valid_head(layout, file, path, &checkpoint).then_some(checkpoint)
}

pub(super) fn load_summary<T>(
    layout: &Layout,
    file: &File,
    path: &Path,
    select: impl FnOnce(JournalSummary, &RawValue) -> Option<T>,
) -> Option<T> {
    let encoded = layout.checkpoint(file).ok()??;
    let checkpoint: Checkpoint<serde::de::IgnoredAny, SummaryFacts, &RawValue> =
        serde_json::from_slice(&encoded).ok()?;
    if !valid_head(layout, file, path, &checkpoint) {
        return None;
    }
    select(checkpoint.facts.summary, checkpoint.runtime)
}

fn valid_head<I, F, R>(
    layout: &Layout,
    file: &File,
    path: &Path,
    checkpoint: &Checkpoint<I, F, R>,
) -> bool {
    if checkpoint.bytes != layout.prefix_bytes
        || checkpoint.head != layout.prefix_head
        || checkpoint.event_count == 0
    {
        return false;
    }
    let end = layout.prefix_bytes;
    reverse::read_before(&mut layout.reader(file, end, 0), path, end, None, 1).is_ok_and(|last| {
        last.first()
            .is_some_and(|event| event.id == checkpoint.head)
    })
}

pub(super) fn encode(checkpoint: &impl Serialize) -> Result<Vec<u8>> {
    let mut encoder = Bounded {
        encoder: GzEncoder::new(Vec::new(), Compression::fast()),
        written: 0,
    };
    match serde_json::to_writer(&mut encoder, checkpoint) {
        Ok(()) => {
            let compressed = encoder.encoder.finish()?;
            Ok(if compressed.len() <= MAX_CHECKPOINT {
                compressed
            } else {
                Vec::new()
            })
        }
        Err(error) if error.io_error_kind() == Some(io::ErrorKind::FileTooLarge) => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

struct Bounded {
    encoder: GzEncoder<Vec<u8>>,
    written: usize,
}

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_CHECKPOINT - self.written {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        let count = self.encoder.write(bytes)?;
        self.written += count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.encoder.flush()
    }
}
