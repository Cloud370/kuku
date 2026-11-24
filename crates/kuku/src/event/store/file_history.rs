use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;

use super::index::FileFact;
use super::{
    EventStore, ReadSource, SeekFrom, StoredEvent, existing_path, read_index, read_source,
};
use crate::{Error, Result};

pub(crate) struct FileHistory<'a> {
    controls: Vec<StoredEvent>,
    owners: HashMap<u64, String>,
    files: Vec<FileFact>,
    reader: super::archive::Reader<'a>,
}

impl EventStore {
    pub(crate) fn with_file_history<T>(
        path: &Path,
        plan: impl FnOnce(FileHistory<'_>) -> Result<T>,
    ) -> Result<T> {
        let path = existing_path(path)?
            .ok_or_else(|| Error::InvalidArgument("Session journal is missing".into()))?;
        let (journal, controls, owners, files) = match read_source(&path)? {
            ReadSource::Active(store) => {
                let writer = store.writer.lock().unwrap();
                (
                    writer.snapshot(&path)?,
                    writer.facts.rewind_controls(),
                    writer.facts.last_turn_owners(),
                    writer.facts.files.clone(),
                )
            }
            ReadSource::Closed(journal) => {
                let (index, _) = read_index(&path, &journal)?;
                (
                    journal,
                    index.rewind_controls(),
                    index.last_turn_owners(),
                    index.files,
                )
            }
        };
        plan(FileHistory {
            controls,
            owners,
            files,
            reader: journal.layout.reader(&journal.file, journal.end, 0),
        })
    }
}

impl FileHistory<'_> {
    pub(crate) fn take_controls(&mut self) -> Vec<StoredEvent> {
        std::mem::take(&mut self.controls)
    }

    pub(crate) fn take_owners(&mut self) -> HashMap<u64, String> {
        std::mem::take(&mut self.owners)
    }

    pub(crate) fn visit(
        &mut self,
        after: u64,
        before: u64,
        mut visit: impl FnMut(StoredEvent) -> Result<()>,
    ) -> Result<()> {
        let facts = &self.files;
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
