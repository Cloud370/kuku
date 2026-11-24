use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::value::RawValue;

use super::{
    Checkpoint, EventStore, HistoryMarks, JournalIndex, JournalSnapshot, JournalSummary, Layout,
    LockedJournal, Replay, ReplayVisitor, Writer, existing_path, read_source, recovery,
    visit_events,
};
use crate::Result;
use crate::util::fs_sync;

use super::archive::PackCheckpoint;

enum Projection {
    Runtime,
    Summary,
    History,
}

#[derive(Default)]
struct ReadFacts {
    history: HistoryMarks,
    summary: JournalSummary,
}

impl EventStore {
    pub(crate) fn needs_packing(&self) -> bool {
        let writer = self.inner.writer.lock().unwrap();
        writer.needs_packing()
    }

    pub(crate) fn visit_recovery(
        &self,
        mut visit: impl FnMut(Replay<'_>) -> Result<bool>,
    ) -> Result<u64> {
        self.inner
            .writer
            .lock()
            .unwrap()
            .visit_recovery(&self.inner.path, &mut visit)
    }

    pub(crate) fn visit_recovery_path(
        path: &Path,
        visit: impl FnMut(Replay<'_>) -> Result<bool>,
    ) -> Result<u64> {
        Self::read_projection(path, Projection::Runtime, visit).map(|(cursor, _)| cursor)
    }

    pub(crate) fn visit_history_path(
        path: &Path,
        visit: impl FnMut(Replay<'_>) -> Result<bool>,
    ) -> Result<HistoryMarks> {
        Self::read_projection(path, Projection::History, visit).map(|(_, facts)| facts.history)
    }

    pub(crate) fn visit_summary_path(
        path: &Path,
        visit: impl FnMut(Replay<'_>) -> Result<bool>,
    ) -> Result<(u64, JournalSummary)> {
        Self::read_projection(path, Projection::Summary, visit)
            .map(|(cursor, facts)| (cursor, facts.summary))
    }

    pub(crate) fn read_sealed_summary<T>(
        path: &Path,
        select: impl FnOnce(JournalSummary, &RawValue) -> Option<T>,
    ) -> Result<Option<T>> {
        let Some(path) = existing_path(path)? else {
            return Ok(None);
        };
        let JournalSnapshot { file, layout, end } = read_source(&path)?.snapshot(&path)?;
        if end != layout.prefix_bytes {
            return Ok(None);
        }
        Ok(recovery::load_summary(&layout, &file, &path, select))
    }

    fn read_projection(
        path: &Path,
        projection: Projection,
        mut visit: impl FnMut(Replay<'_>) -> Result<bool>,
    ) -> Result<(u64, ReadFacts)> {
        let mut result = ReadFacts::default();
        let Some(path) = existing_path(path)? else {
            return Ok((0, result));
        };
        let JournalSnapshot { file, layout, end } = read_source(&path)?.snapshot(&path)?;
        let mut facts = matches!(projection, Projection::History).then(JournalIndex::default);
        let (mut start, mut after) = (0, 0);
        if let Some(saved) = recovery::load(&layout, &file, &path) {
            if visit(Replay::Checkpoint(&saved.runtime))? {
                start = layout.prefix_bytes;
                after = layout.prefix_head;
                match projection {
                    Projection::History => facts = Some(saved.facts),
                    Projection::Summary => result.summary = saved.facts.summary,
                    Projection::Runtime => {}
                }
            }
        }
        let cursor = visit_events(
            layout.reader(&file, end, start),
            &path,
            start,
            after,
            |event| {
                if let Some(facts) = &mut facts {
                    facts.observe(&event);
                }
                if matches!(projection, Projection::Summary) {
                    result.summary.observe(&event);
                }
                visit(Replay::Event(event)).map(|_| ())
            },
        )?;
        if let Some(facts) = facts {
            result.history = facts.history_marks(true, true, cursor);
        }
        Ok((cursor, result))
    }

    pub(crate) fn pack_closed(&self, head: u64, runtime: &impl Serialize) -> Result<bool> {
        let path = &self.inner.path;
        let mut writer = self.inner.writer.lock().unwrap();
        writer.ensure_healthy(path)?;
        if writer.last_id != head || !writer.needs_packing() {
            return Ok(false);
        }
        writer.pack(path, runtime)
    }
}

impl Writer {
    fn needs_packing(&self) -> bool {
        self.refresh_checkpoint || self.layout.worth_packing(self.committed_len)
    }

    pub(super) fn visit_recovery(
        &mut self,
        path: &Path,
        visit: &mut ReplayVisitor<'_>,
    ) -> Result<u64> {
        self.ensure_healthy(path)?;
        let file = &self.journal.as_ref().unwrap().file;
        let mut after = 0;
        if let Some(checkpoint) = recovery::load(&self.layout, file, path) {
            if visit(Replay::Checkpoint(&checkpoint.runtime))? {
                after = checkpoint.head;
            }
        }
        self.visit_after(path, after, |event| visit(Replay::Event(event)).map(|_| ()))
    }

    pub(super) fn pack(&mut self, path: &Path, runtime: &impl Serialize) -> Result<bool> {
        let checkpoint = recovery::encode(&Checkpoint {
            head: self.last_id,
            bytes: self.committed_len,
            event_count: self.event_count,
            index: &self.index,
            facts: &self.facts,
            runtime,
        })?;
        let Some(replacement) = self.layout.pack(
            &self.journal.as_ref().unwrap().file,
            path,
            self.committed_len,
            self.last_id,
            self.turn_count,
            PackCheckpoint {
                bytes: &checkpoint,
                replace_invalid: self.refresh_checkpoint && !checkpoint.is_empty(),
            },
        )?
        else {
            return Ok(false);
        };
        drop(self.journal.take());
        let publish = fs_sync::replace_journal(&replacement.0, path);
        let reopen = (|| {
            let file = fs_sync::write_options().read(true).open(path)?;
            let journal = LockedJournal::writer(file, path)?;
            let layout = Layout::read(&journal.file)?;
            Ok::<_, crate::Error>((journal, layout))
        })();
        match reopen {
            Ok((journal, layout)) => {
                self.journal = Some(journal);
                self.layout = Arc::new(layout);
            }
            Err(error) => {
                self.poison = Some(error.to_string());
                return Err(error);
            }
        }
        publish?;
        self.refresh_checkpoint = false;
        Ok(true)
    }
}
