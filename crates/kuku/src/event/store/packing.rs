use std::path::Path;

use serde::Serialize;

use super::{
    acquire_file_lock, existing_path, find_active_store, recovery, registry, visit_events,
    Checkpoint, EventStore, Layout, LockedJournal, Replay, ReplayVisitor, Writer,
};
use crate::util::fs_sync;
use crate::Result;

impl EventStore {
    pub(crate) fn needs_packing(&self) -> bool {
        let writer = self.inner.writer.lock().unwrap();
        writer.layout.worth_packing(writer.committed_len)
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
        mut visit: impl FnMut(Replay<'_>) -> Result<bool>,
    ) -> Result<u64> {
        let Some(path) = existing_path(path)? else {
            return Ok(0);
        };
        let stores = registry().lock().unwrap();
        if let Some(inner) = find_active_store(&stores, &path) {
            drop(stores);
            return inner
                .writer
                .lock()
                .unwrap()
                .visit_recovery(&path, &mut visit);
        }
        let _lock = acquire_file_lock(&path)?;
        let journal = LockedJournal::reader(&path)?;
        let layout = Layout::read(&journal.file)?;
        let end = layout.logical_len(journal.file.metadata()?.len());
        let (mut start, mut after) = (0, 0);
        if let Some(saved) = recovery::load(&layout, &journal.file, &path) {
            if visit(Replay::Checkpoint(&saved.runtime))? {
                start = layout.prefix_bytes;
                after = layout.prefix_head;
            }
        }
        visit_events(
            layout.reader(&journal.file, end, start),
            &path,
            start,
            after,
            |event| visit(Replay::Event(event)).map(|_| ()),
        )
    }

    pub(crate) fn pack_closed(&self, head: u64, runtime: &impl Serialize) -> Result<bool> {
        let path = &self.inner.path;
        let mut writer = self.inner.writer.lock().unwrap();
        writer.ensure_healthy(path)?;
        if writer.last_id != head || !writer.layout.worth_packing(writer.committed_len) {
            return Ok(false);
        }
        writer.pack(path, runtime)
    }
}

impl Writer {
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
            &checkpoint,
        )?
        else {
            return Ok(false);
        };
        drop(self.journal.take());
        let publish = fs_sync::rename(&replacement.0, path);
        let reopen = (|| {
            let file = fs_sync::write_options().read(true).open(path)?;
            let journal = LockedJournal::writer(file, path)?;
            let layout = Layout::read(&journal.file)?;
            Ok::<_, crate::Error>((journal, layout))
        })();
        match reopen {
            Ok((journal, layout)) => {
                self.journal = Some(journal);
                self.layout = layout;
            }
            Err(error) => {
                self.poison = Some(error.to_string());
                return Err(error);
            }
        }
        publish?;
        Ok(true)
    }
}
