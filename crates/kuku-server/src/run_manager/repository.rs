use std::path::{Path, PathBuf};

use kuku::event::{EventPayload, StoredEvent, TaskId, TaskLedgerRecord};

use super::domain::{DomainError, TaskAggregate};

/// Durable per-Task event ledger repository.
#[derive(Debug, Clone)]
pub struct TaskRepository {
    root: PathBuf,
}

impl TaskRepository {
    pub fn open(home: impl AsRef<Path>) -> Result<Self, DomainError> {
        let root = home.as_ref().join("tasks");
        std::fs::create_dir_all(&root).map_err(|_| DomainError::LedgerCorrupt)?;
        Ok(Self { root })
    }

    pub fn task_path(&self, task_id: &TaskId) -> PathBuf {
        self.root.join(task_id.as_str())
    }

    pub fn events_path(&self, task_id: &TaskId) -> PathBuf {
        self.task_path(task_id).join("events.jsonl")
    }

    pub fn append(
        &self,
        task_id: &TaskId,
        record: TaskLedgerRecord,
    ) -> Result<StoredEvent, DomainError> {
        let path = self.events_path(task_id);
        let mut store =
            kuku::event::EventStore::open(path).map_err(|_| DomainError::LedgerCorrupt)?;
        store
            .append_synced(EventPayload::TaskLedger(record))
            .map_err(|_| DomainError::LedgerCorrupt)
    }

    pub fn replay(&self, task_id: &TaskId) -> Result<Vec<StoredEvent>, DomainError> {
        kuku::event::EventStore::replay(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)
    }

    pub fn rebuild(&self, task_id: &TaskId) -> Result<TaskAggregate, DomainError> {
        let mut aggregate = TaskAggregate::default();
        for event in self.replay(task_id)? {
            if let EventPayload::TaskLedger(record) = event.payload {
                aggregate.apply_record(
                    kuku::event::Cursor::try_new(event.id)
                        .map_err(|_| DomainError::StorageExhausted)?,
                    &record,
                )?;
            }
        }
        Ok(aggregate)
    }
}
