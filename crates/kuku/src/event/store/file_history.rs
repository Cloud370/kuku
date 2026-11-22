use super::{
    existing_path, find_active_store, read_index, registry, EventStore, JournalIndex, Layout,
    LockedJournal, SeekFrom, StoredEvent,
};
use crate::{Error, Result};
use std::io::{Read, Seek};
use std::path::Path;

pub(crate) struct FileHistory<'a> {
    index: &'a JournalIndex,
    reader: super::archive::Reader<'a>,
}

impl EventStore {
    pub(crate) fn with_file_history<T>(
        path: &Path,
        plan: impl FnOnce(FileHistory<'_>) -> Result<T>,
    ) -> Result<T> {
        let path = existing_path(path)?
            .ok_or_else(|| Error::InvalidArgument("Session journal is missing".into()))?;
        let stores = registry().lock().unwrap();
        if let Some(store) = find_active_store(&stores, &path) {
            drop(stores);
            let writer = store.writer.lock().unwrap();
            writer.ensure_healthy(&path)?;
            let file = &writer.journal.as_ref().unwrap().file;
            return plan(FileHistory {
                index: &writer.facts,
                reader: writer.layout.reader(file, writer.committed_len, 0),
            });
        }
        let journal = LockedJournal::reader(&path)?;
        drop(stores);
        let layout = Layout::read(&journal.file)?;
        let end = layout.logical_len(journal.file.metadata()?.len());
        let (index, _) = read_index(&path, &journal.file)?;
        plan(FileHistory {
            index: &index,
            reader: layout.reader(&journal.file, end, 0),
        })
    }
}

impl FileHistory<'_> {
    pub(crate) fn controls(&self) -> Vec<StoredEvent> {
        self.index.rewind_controls()
    }

    pub(crate) fn owners(&self) -> std::collections::HashMap<u64, String> {
        self.index.last_turn_owners()
    }

    pub(crate) fn visit(
        &mut self,
        after: u64,
        before: u64,
        mut visit: impl FnMut(StoredEvent) -> Result<()>,
    ) -> Result<()> {
        let facts = &self.index.files;
        let start = facts.partition_point(|fact| fact.location.id <= after);
        let end = facts.partition_point(|fact| fact.location.id < before);
        let mut buffer = Vec::new();
        for fact in &facts[start..end.max(start)] {
            self.reader.seek(SeekFrom::Start(fact.location.offset))?;
            buffer.resize(fact.bytes, 0);
            self.reader.read_exact(&mut buffer)?;
            visit(serde_json::from_slice(&buffer)?)?;
        }
        Ok(())
    }
}
