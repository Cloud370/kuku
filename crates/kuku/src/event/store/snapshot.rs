use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use crate::util::fs_sync;
use crate::Result;

use super::{find_active_store, recovery, reverse, store_slot, Layout, StoreInner, Writer};

pub(super) struct LockedJournal {
    pub(super) file: File,
}

impl LockedJournal {
    pub(super) fn writer(file: File, path: &Path) -> Result<Self> {
        fs_sync::lock_journal(&file, path, true)?;
        Ok(Self { file })
    }

    pub(super) fn reader(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        fs_sync::lock_journal(&file, path, false)?;
        Ok(Self { file })
    }
}

impl Drop for LockedJournal {
    fn drop(&mut self) {
        // NOTE: A duplicated or inherited handle can outlive this owner.
        let _ = fs_sync::unlock_journal(&self.file);
    }
}

pub(super) struct JournalSnapshot {
    pub(super) file: File,
    pub(super) layout: Arc<Layout>,
    pub(super) end: u64,
}

impl JournalSnapshot {
    pub(super) fn closed(path: &Path) -> Result<Self> {
        let journal = LockedJournal::reader(path)?;
        let layout = Layout::read(&journal.file)?;
        let end = committed_end(&journal.file, &layout)?;
        Ok(Self {
            file: journal.file.try_clone()?,
            layout: Arc::new(layout),
            end,
        })
    }
}

impl Writer {
    pub(super) fn snapshot(&self, path: &Path) -> Result<JournalSnapshot> {
        self.ensure_healthy(path)?;
        Ok(JournalSnapshot {
            file: File::open(path)?,
            layout: self.layout.clone(),
            end: self.committed_len,
        })
    }
}

fn committed_end(mut file: &File, layout: &Layout) -> io::Result<u64> {
    let tail = layout.tail_offset(layout.prefix_bytes)?;
    let mut end = file.metadata()?.len();
    let mut bytes = [0; 8192];
    while end > tail {
        let len = (end - tail).min(bytes.len() as u64) as usize;
        let start = end - len as u64;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut bytes[..len])?;
        if let Some(newline) = bytes[..len].iter().rposition(|byte| *byte == b'\n') {
            return Ok(layout.logical_len(start + newline as u64 + 1));
        }
        end = start;
    }
    Ok(layout.prefix_bytes)
}

pub(super) enum ReadSource {
    Active(Arc<StoreInner>),
    Closed(JournalSnapshot),
}

impl ReadSource {
    pub(super) fn snapshot(self, path: &Path) -> Result<JournalSnapshot> {
        match self {
            Self::Active(inner) => inner.writer.lock().unwrap().snapshot(path),
            Self::Closed(snapshot) => Ok(snapshot),
        }
    }
}

pub(super) fn read_source(path: &Path) -> Result<ReadSource> {
    let slot = store_slot(path);
    let entry = slot.lock().unwrap();
    match find_active_store(entry.as_ref()) {
        Some(inner) => Ok(ReadSource::Active(inner)),
        None => Ok(ReadSource::Closed(JournalSnapshot::closed(path)?)),
    }
}

pub(super) fn read_after_source(path: &Path, after: u64) -> Result<(JournalSnapshot, u64)> {
    let (journal, start) = match read_source(path)? {
        ReadSource::Active(inner) => {
            let writer = inner.writer.lock().unwrap();
            (writer.snapshot(path)?, writer.offset_at(after))
        }
        ReadSource::Closed(journal) => {
            let start = recovery::page_start(&journal.layout, &journal.file, path, after);
            (journal, start)
        }
    };
    let start = if after >= journal.layout.prefix_head {
        start.max(journal.layout.prefix_bytes)
    } else {
        start
    };
    Ok((journal, start))
}

pub(super) fn read_before_source(
    path: &Path,
    before: Option<u64>,
) -> Result<(JournalSnapshot, u64)> {
    match read_source(path)? {
        ReadSource::Active(inner) => {
            let writer = inner.writer.lock().unwrap();
            Ok((writer.snapshot(path)?, writer.offset_before(before)))
        }
        ReadSource::Closed(journal) => {
            let layout = &journal.layout;
            let end = journal.end;
            let before =
                before.filter(|id| end != layout.prefix_bytes || *id <= layout.prefix_head);
            let bound = before.map_or(Ok(end), |id| {
                let mut reader = layout.reader(&journal.file, end, 0);
                reverse::find_page_end(&mut reader, path, end, id)
            })?;
            Ok((journal, bound))
        }
    }
}
