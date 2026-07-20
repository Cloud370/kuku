use std::collections::HashMap;
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use kuku::event::{EventPayload, StoredEvent, TaskId, TaskLedgerRecord};
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};

use crate::api::TaskSummary;

use super::domain::{DomainError, TaskAggregate};
use super::idempotency::{DurableReceipt, IdempotencyIndex};

#[derive(Debug, Default)]
struct RepositoryIndexes {
    receipts: IdempotencyIndex,
    summaries: HashMap<TaskId, TaskSummary>,
}

#[derive(Debug, Default)]
struct RepositoryState {
    create_gate: Arc<TokioMutex<()>>,
    scan_gate: Mutex<()>,
    task_gates: Mutex<HashMap<TaskId, Weak<TokioMutex<()>>>>,
    key_gates: Mutex<HashMap<String, Weak<TokioMutex<()>>>>,
    indexes: Mutex<RepositoryIndexes>,
}

/// Durable per-Task event ledger repository.
#[derive(Debug, Clone)]
pub struct TaskRepository {
    root: PathBuf,
    state: Arc<RepositoryState>,
}

impl TaskRepository {
    pub fn open(home: impl AsRef<Path>) -> Result<Self, DomainError> {
        std::fs::create_dir_all(home.as_ref()).map_err(|_| DomainError::LedgerCorrupt)?;
        let home = std::fs::canonicalize(home).map_err(|_| DomainError::LedgerCorrupt)?;
        let root = home.join("tasks");
        std::fs::create_dir_all(&root).map_err(|_| DomainError::LedgerCorrupt)?;
        sync_directory(&root)?;
        sync_directory(&home)?;
        let state = shared_state(&root);
        let repository = Self { root, state };
        let _scan = repository
            .state
            .scan_gate
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        let indexes = repository.scan_indexes()?;
        *repository
            .state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)? = indexes;
        drop(_scan);
        Ok(repository)
    }

    pub fn task_path(&self, task_id: &TaskId) -> PathBuf {
        debug_assert!(valid_task_component(task_id));
        self.root.join(task_id.as_str())
    }

    pub fn events_path(&self, task_id: &TaskId) -> PathBuf {
        self.task_path(task_id).join("events.jsonl")
    }

    pub async fn create_guard(&self) -> OwnedMutexGuard<()> {
        self.state.create_gate.clone().lock_owned().await
    }

    pub async fn task_guard(&self, task_id: &TaskId) -> OwnedMutexGuard<()> {
        shared_gate(&self.state.task_gates, task_id.clone())
            .lock_owned()
            .await
    }

    pub async fn key_guard(&self, key: &str) -> OwnedMutexGuard<()> {
        shared_gate(&self.state.key_gates, key.to_owned())
            .lock_owned()
            .await
    }

    pub fn receipt(&self, key: &str, digest: &str) -> Result<Option<DurableReceipt>, DomainError> {
        self.state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .receipts
            .lookup(key, digest)
    }

    pub fn append_initial(
        &self,
        task_id: &TaskId,
        record: TaskLedgerRecord,
    ) -> Result<StoredEvent, DomainError> {
        let _scan = self
            .state
            .scan_gate
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        if !valid_task_component(task_id) {
            return Err(DomainError::LedgerCorrupt);
        }
        let task_path = self.task_path(task_id);
        std::fs::create_dir(&task_path).map_err(|_| DomainError::LedgerCorrupt)?;
        let result = self.append_to_existing(task_id, record);
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&task_path);
            return result;
        }
        sync_directory(&task_path)?;
        sync_directory(&self.root)?;
        result
    }

    pub fn append(
        &self,
        task_id: &TaskId,
        record: TaskLedgerRecord,
    ) -> Result<StoredEvent, DomainError> {
        let _scan = self
            .state
            .scan_gate
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        if !self.task_path(task_id).is_dir() {
            return Err(DomainError::TaskNotFound);
        }
        self.append_to_existing(task_id, record)
    }

    fn append_to_existing(
        &self,
        task_id: &TaskId,
        record: TaskLedgerRecord,
    ) -> Result<StoredEvent, DomainError> {
        let mut store = kuku::event::EventStore::open(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)?;
        store
            .append_synced(EventPayload::TaskLedger(record))
            .map_err(|_| DomainError::LedgerCorrupt)
    }

    pub fn publish(&self, task_id: &TaskId) -> Result<TaskAggregate, DomainError> {
        let _scan = self
            .state
            .scan_gate
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        let aggregate = self.rebuild(task_id)?;
        let mut indexes = self
            .state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        let events = self.replay(task_id)?;
        let Some(transaction) = events.iter().rev().find_map(control_transaction) else {
            return Err(DomainError::LedgerCorrupt);
        };
        if indexes
            .receipts
            .lookup(
                transaction.command().idempotency_key(),
                transaction.command().intent_digest(),
            )?
            .is_none()
        {
            indexes.receipts.insert(
                transaction.command().idempotency_key().to_owned(),
                DurableReceipt {
                    digest: transaction.command().intent_digest().to_owned(),
                    task_id: task_id.clone(),
                    task_revision: transaction.task_revision(),
                    result: transaction.command().result().clone(),
                },
            )?;
        }
        indexes
            .summaries
            .insert(task_id.clone(), aggregate.summary());
        Ok(aggregate)
    }

    pub fn replay(&self, task_id: &TaskId) -> Result<Vec<StoredEvent>, DomainError> {
        if !valid_task_component(task_id) || !self.task_path(task_id).is_dir() {
            return Err(DomainError::TaskNotFound);
        }
        kuku::event::EventStore::replay(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)
    }

    pub fn rebuild(&self, task_id: &TaskId) -> Result<TaskAggregate, DomainError> {
        self.rebuild_through(task_id, None)
    }

    pub fn rebuild_at(&self, task_id: &TaskId, cursor: u64) -> Result<TaskAggregate, DomainError> {
        self.rebuild_through(task_id, Some(cursor))
    }

    fn rebuild_through(
        &self,
        task_id: &TaskId,
        through: Option<u64>,
    ) -> Result<TaskAggregate, DomainError> {
        let mut aggregate = TaskAggregate::default();
        for event in self.replay(task_id)? {
            if through.is_some_and(|cursor| event.id > cursor) {
                break;
            }
            if let EventPayload::TaskLedger(record) = event.payload {
                aggregate.apply_record(
                    kuku::event::Cursor::try_new(event.id)
                        .map_err(|_| DomainError::StorageExhausted)?,
                    &record,
                )?;
            }
        }
        if aggregate.task_id() != Some(task_id) {
            return Err(DomainError::LedgerCorrupt);
        }
        aggregate.set_updated_at(self.updated_at(task_id)?);
        Ok(aggregate)
    }

    pub fn summaries(&self) -> Result<Vec<TaskSummary>, DomainError> {
        Ok(self
            .state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .summaries
            .values()
            .cloned()
            .collect())
    }

    pub fn task_ids(&self) -> Result<Vec<TaskId>, DomainError> {
        Ok(self
            .state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .summaries
            .keys()
            .cloned()
            .collect())
    }

    fn scan_indexes(&self) -> Result<RepositoryIndexes, DomainError> {
        let mut indexes = RepositoryIndexes::default();
        for entry in std::fs::read_dir(&self.root).map_err(|_| DomainError::LedgerCorrupt)? {
            let entry = entry.map_err(|_| DomainError::LedgerCorrupt)?;
            if !entry
                .file_type()
                .map_err(|_| DomainError::LedgerCorrupt)?
                .is_dir()
            {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| DomainError::LedgerCorrupt)?;
            let task_id = TaskId::parse(name).map_err(|_| DomainError::LedgerCorrupt)?;
            let aggregate = self.rebuild(&task_id)?;
            for event in self.replay(&task_id)? {
                if let Some(transaction) = control_transaction(&event) {
                    indexes.receipts.insert(
                        transaction.command().idempotency_key().to_owned(),
                        DurableReceipt {
                            digest: transaction.command().intent_digest().to_owned(),
                            task_id: task_id.clone(),
                            task_revision: transaction.task_revision(),
                            result: transaction.command().result().clone(),
                        },
                    )?;
                }
            }
            indexes.summaries.insert(task_id, aggregate.summary());
        }
        Ok(indexes)
    }

    fn updated_at(&self, task_id: &TaskId) -> Result<String, DomainError> {
        let modified = std::fs::metadata(self.events_path(task_id))
            .and_then(|metadata| metadata.modified())
            .map_err(|_| DomainError::LedgerCorrupt)?;
        super::domain::system_time_rfc3339(modified)
    }
}

fn control_transaction(event: &StoredEvent) -> Option<&kuku::event::TaskTransaction> {
    match &event.payload {
        EventPayload::TaskLedger(TaskLedgerRecord::Control(transaction)) => Some(transaction),
        _ => None,
    }
}

fn valid_task_component(task_id: &TaskId) -> bool {
    let mut components = Path::new(task_id.as_str()).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn sync_directory(path: &Path) -> Result<(), DomainError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DomainError::LedgerCorrupt)
}

fn shared_state(root: &Path) -> Arc<RepositoryState> {
    static STATES: OnceLock<Mutex<HashMap<PathBuf, Weak<RepositoryState>>>> = OnceLock::new();
    let states = STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states
        .lock()
        .expect("repository state lock is not poisoned");
    if let Some(state) = states.get(root).and_then(Weak::upgrade) {
        return state;
    }
    let state = Arc::new(RepositoryState::default());
    states.insert(root.to_owned(), Arc::downgrade(&state));
    state
}

fn shared_gate<K>(gates: &Mutex<HashMap<K, Weak<TokioMutex<()>>>>, key: K) -> Arc<TokioMutex<()>>
where
    K: std::hash::Hash + Eq,
{
    let mut gates = gates.lock().expect("repository gate map is not poisoned");
    if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(TokioMutex::new(()));
    gates.insert(key, Arc::downgrade(&gate));
    gate
}
