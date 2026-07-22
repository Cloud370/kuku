use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::Barrier;
use std::sync::{Arc, Mutex, Weak};

use kuku::event::{EventPayload, StoredEvent, TaskId, TaskLedgerRecord};
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};

use crate::api::{TaskDelta, TaskProjection, TaskSummary};

use super::domain::{DomainError, TaskAggregate};
use super::idempotency::{DurableReceipt, IdempotencyIndex};
use super::repository_support::{
    shared_gate, shared_state, sync_directory, updated_at, valid_task_component,
};

#[derive(Debug, Default)]
struct RepositoryIndexes {
    receipts: IdempotencyIndex,
    summaries: HashMap<TaskId, TaskSummary>,
}

#[derive(Default)]
pub(super) struct RepositoryState {
    create_gate: Arc<TokioMutex<()>>,
    scan_gate: Mutex<()>,
    task_gates: Mutex<HashMap<TaskId, Weak<TokioMutex<()>>>>,
    key_gates: Mutex<HashMap<String, Weak<TokioMutex<()>>>>,
    indexes: Mutex<RepositoryIndexes>,
    aggregates: Mutex<HashMap<TaskId, TaskAggregate>>,
    observed_tasks: Mutex<HashSet<TaskId>>,
    publication_results: Mutex<HashMap<(TaskId, u64), Result<TaskPublication, DomainError>>>,
    awaited_records: Mutex<HashMap<TaskId, TaskLedgerRecord>>,
    publication_observers: Mutex<Vec<TaskPublicationObserver>>,
    dirty_tasks: Mutex<HashSet<TaskId>>,
    dirty_keys: Mutex<HashMap<String, TaskId>>,
    unconfirmed_tasks: Mutex<HashSet<TaskId>>,
    #[cfg(test)]
    append_failures: AtomicUsize,
    #[cfg(test)]
    fail_next_publication: AtomicBool,
    #[cfg(test)]
    fail_next_cache: AtomicBool,
    #[cfg(test)]
    fail_next_store_after_write: AtomicBool,
    #[cfg(test)]
    suppress_next_publication: AtomicBool,
    #[cfg(test)]
    durability_confirmation_failures: AtomicUsize,
    #[cfg(test)]
    repair_snapshot_pause: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
}

type TaskPublicationObserver = Arc<dyn Fn(&TaskPublication) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct TaskPublication {
    pub event: StoredEvent,
    pub projection: TaskProjection,
    pub delta: TaskDelta,
}

/// Durable per-Task event ledger repository.
#[derive(Clone)]
pub struct TaskRepository {
    root: PathBuf,
    state: Arc<RepositoryState>,
}

impl std::fmt::Debug for TaskRepository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TaskRepository")
            .field("root", &self.root)
            .finish()
    }
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
        let (indexes, aggregates) = repository.scan_indexes()?;
        let task_ids = aggregates.keys().cloned().collect::<Vec<_>>();
        *repository
            .state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)? = indexes;
        *repository
            .state
            .aggregates
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)? = aggregates;
        repository.rewrite_projection_caches()?;
        for task_id in task_ids {
            repository.ensure_task_observer(&task_id)?;
        }
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

    pub fn event_store(&self, task_id: &TaskId) -> Result<kuku::event::EventStore, DomainError> {
        kuku::event::EventStore::open(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)
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
        let dirty_task = self
            .state
            .dirty_keys
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .get(key)
            .cloned();
        if let Some(task_id) = dirty_task {
            self.repair_task(&task_id)?;
        }
        self.state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .receipts
            .lookup(key, digest)
    }

    pub fn register_observer(&self, observer: TaskPublicationObserver) {
        self.state
            .publication_observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(observer);
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
        self.validate_record(task_id, &record, true)?;
        let task_path = self.task_path(task_id);
        std::fs::create_dir(&task_path).map_err(|_| DomainError::LedgerCorrupt)?;
        kuku::event::EventStore::open(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)?;
        sync_directory(&task_path)?;
        sync_directory(&self.root)?;
        let result = self.append_to_existing(task_id, record);
        if result.is_err() {
            let has_record = kuku::event::EventStore::replay(self.events_path(task_id))
                .is_ok_and(|events| !events.is_empty());
            if !has_record {
                let _ = std::fs::remove_dir_all(&task_path);
            }
            return result;
        }
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
        self.validate_record(task_id, &record, false)?;
        self.append_to_existing(task_id, record)
    }

    fn validate_record(
        &self,
        task_id: &TaskId,
        record: &TaskLedgerRecord,
        initial: bool,
    ) -> Result<(), DomainError> {
        let mut aggregate = if initial {
            TaskAggregate::default()
        } else {
            let dirty = self
                .state
                .dirty_tasks
                .lock()
                .map_err(|_| DomainError::LedgerCorrupt)?
                .contains(task_id);
            let cached = self
                .state
                .aggregates
                .lock()
                .map_err(|_| DomainError::LedgerCorrupt)?
                .get(task_id)
                .cloned();
            match (dirty, cached) {
                (false, Some(aggregate)) => aggregate,
                _ => self.rebuild(task_id)?,
            }
        };
        let cursor = aggregate
            .cursor()
            .get()
            .checked_add(1)
            .and_then(|cursor| kuku::event::Cursor::try_new(cursor).ok())
            .ok_or(DomainError::StorageExhausted)?;
        let delta = super::projection::reduce_record(&mut aggregate, cursor, record)?;
        super::domain::ensure_bounded(&delta)?;
        if let TaskDelta::ChangesApplied { changes, .. } = delta {
            aggregate.validate_timeline_changes(&changes)?;
        }
        let events = match record {
            TaskLedgerRecord::Control(transaction) => transaction.events(),
            TaskLedgerRecord::Activity(batch) => batch.events(),
        };
        for event in events {
            if let kuku::event::TaskEvent::ReviewSubmissionRecorded(recorded) = event {
                super::domain::ensure_bounded(&super::submission::review_result(
                    recorded.clone(),
                    false,
                ))?;
            }
        }
        Ok(())
    }

    fn append_to_existing(
        &self,
        task_id: &TaskId,
        record: TaskLedgerRecord,
    ) -> Result<StoredEvent, DomainError> {
        #[cfg(test)]
        if self
            .state
            .append_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(DomainError::LedgerCorrupt);
        }
        self.ensure_task_observer(task_id)?;
        self.state
            .awaited_records
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .insert(task_id.clone(), record.clone());
        let mut store = kuku::event::EventStore::open(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)?;
        #[cfg(test)]
        let inject_late_error = self
            .state
            .fail_next_store_after_write
            .swap(false, Ordering::SeqCst);
        #[cfg(test)]
        if inject_late_error {
            self.state
                .suppress_next_publication
                .store(true, Ordering::SeqCst);
        }
        let expected = record.clone();
        let event = match store.append_synced(EventPayload::TaskLedger(record)) {
            Ok(event) => event,
            Err(_) => {
                self.state
                    .awaited_records
                    .lock()
                    .map_err(|_| DomainError::LedgerCorrupt)?
                    .remove(task_id);
                self.recover_failed_record(task_id, &expected)?;
                return Err(DomainError::LedgerCorrupt);
            }
        };
        #[cfg(test)]
        if inject_late_error {
            self.state
                .awaited_records
                .lock()
                .map_err(|_| DomainError::LedgerCorrupt)?
                .remove(task_id);
            self.recover_failed_record(task_id, &expected)?;
            return Err(DomainError::LedgerCorrupt);
        }
        let publication = self
            .state
            .publication_results
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .remove(&(task_id.clone(), event.id))
            .ok_or(DomainError::LedgerCorrupt)?;
        publication.map(|_| event)
    }

    fn ensure_task_observer(&self, task_id: &TaskId) -> Result<(), DomainError> {
        let mut observed = self
            .state
            .observed_tasks
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        if observed.contains(task_id) {
            return Ok(());
        }
        let store = kuku::event::EventStore::open(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)?;
        let repository = self.clone();
        let task_id = task_id.clone();
        let observed_task_id = task_id.clone();
        store.register_observer(Arc::new(move |event| {
            #[cfg(test)]
            if repository
                .state
                .suppress_next_publication
                .swap(false, Ordering::SeqCst)
            {
                return;
            }
            let result = repository.publish_stored(&task_id, event.clone());
            if result.is_err() {
                repository.mark_dirty(&task_id, event);
            }
            let awaited = match &event.payload {
                EventPayload::TaskLedger(record) => {
                    let mut awaited = repository
                        .state
                        .awaited_records
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if awaited.get(&task_id) == Some(record) {
                        awaited.remove(&task_id);
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if awaited {
                repository
                    .state
                    .publication_results
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert((task_id.clone(), event.id), result);
            }
        }));
        observed.insert(observed_task_id);
        Ok(())
    }

    fn recover_failed_record(
        &self,
        task_id: &TaskId,
        record: &TaskLedgerRecord,
    ) -> Result<(), DomainError> {
        let event = kuku::event::EventStore::replay(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)?
            .into_iter()
            .rev()
            .find(|event| {
                matches!(&event.payload, EventPayload::TaskLedger(stored) if stored == record)
            });
        if let Some(event) = event {
            if self.confirm_durability(task_id).is_ok() {
                self.mark_dirty(task_id, &event);
            } else {
                self.mark_unconfirmed(task_id, &event);
            }
        }
        Ok(())
    }

    fn confirm_durability(&self, task_id: &TaskId) -> Result<(), DomainError> {
        #[cfg(test)]
        if self
            .state
            .durability_confirmation_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(DomainError::LedgerCorrupt);
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.events_path(task_id))
            .and_then(|file| file.sync_data())
            .map_err(|_| DomainError::LedgerCorrupt)
    }

    fn publish_stored(
        &self,
        task_id: &TaskId,
        event: StoredEvent,
    ) -> Result<TaskPublication, DomainError> {
        #[cfg(test)]
        if self
            .state
            .fail_next_publication
            .swap(false, Ordering::SeqCst)
        {
            return Err(DomainError::LedgerCorrupt);
        }
        let cursor =
            kuku::event::Cursor::try_new(event.id).map_err(|_| DomainError::StorageExhausted)?;
        let dirty = self
            .state
            .dirty_tasks
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .contains(task_id);
        let cached = self
            .state
            .aggregates
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .get(task_id)
            .cloned();
        let cache_missing = cached.is_none();
        let needs_rebuild = dirty
            || cached
                .as_ref()
                .is_some_and(|aggregate| aggregate.cursor().get().checked_add(1) != Some(event.id));
        let mut aggregate = if needs_rebuild {
            let previous = event.id.checked_sub(1).ok_or(DomainError::LedgerCorrupt)?;
            if previous == 0 {
                TaskAggregate::default()
            } else {
                self.merge_task_receipts(task_id, Some(previous))?;
                self.rebuild_at(task_id, previous)?
            }
        } else {
            cached.unwrap_or_default()
        };
        let mut delta = match &event.payload {
            EventPayload::TaskLedger(record) => {
                super::projection::reduce_record(&mut aggregate, cursor, record)?
            }
            _ => {
                aggregate.advance_cursor(cursor);
                TaskDelta::ChangesApplied {
                    changes: Vec::new(),
                    timeline_window: None,
                }
            }
        };
        let force_replacement = match &delta {
            TaskDelta::ChangesApplied { changes, .. } => {
                match aggregate
                    .validate_timeline_changes(changes)
                    .and_then(|()| super::domain::ensure_bounded(&delta))
                {
                    Ok(()) => false,
                    Err(DomainError::PayloadTooLarge) => true,
                    Err(error) => return Err(error),
                }
            }
            TaskDelta::ProjectionReplaced { .. } => false,
        };
        aggregate.set_updated_at(updated_at(&self.events_path(task_id))?);
        let projection = aggregate.projection()?;
        if dirty || cache_missing || force_replacement {
            delta = TaskDelta::ProjectionReplaced {
                projection: Box::new(projection.clone()),
            };
        }

        let mut indexes = self
            .state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        if let EventPayload::TaskLedger(TaskLedgerRecord::Control(transaction)) = &event.payload {
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
        drop(indexes);
        self.state
            .aggregates
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .insert(task_id.clone(), aggregate);

        self.write_projection(task_id, &projection)?;
        self.clear_dirty(task_id)?;

        let publication = TaskPublication {
            event,
            projection,
            delta,
        };
        if delta_has_ui_change(&publication.delta) {
            self.notify_observers(&publication)?;
        }
        Ok(publication)
    }

    fn notify_observers(&self, publication: &TaskPublication) -> Result<(), DomainError> {
        let observers = self
            .state
            .publication_observers
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .clone();
        for observer in observers {
            observer(publication);
        }
        Ok(())
    }

    pub fn replay(&self, task_id: &TaskId) -> Result<Vec<StoredEvent>, DomainError> {
        if !valid_task_component(task_id) || !self.task_path(task_id).is_dir() {
            return Err(DomainError::TaskNotFound);
        }
        kuku::event::EventStore::replay(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)
    }

    pub fn review_submission(
        &self,
        task_id: &TaskId,
        submission_id: &kuku::event::ReviewSubmissionId,
    ) -> Result<kuku::event::ReviewSubmissionRecorded, DomainError> {
        self.replay(task_id)?
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::TaskLedger(TaskLedgerRecord::Control(transaction)) => {
                    Some(transaction)
                }
                _ => None,
            })
            .flat_map(|transaction| transaction.events().to_vec())
            .find_map(|event| match event {
                kuku::event::TaskEvent::ReviewSubmissionRecorded(recorded)
                    if &recorded.submission_id == submission_id =>
                {
                    Some(recorded)
                }
                _ => None,
            })
            .ok_or(DomainError::LedgerCorrupt)
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
            let cursor = kuku::event::Cursor::try_new(event.id)
                .map_err(|_| DomainError::StorageExhausted)?;
            if let EventPayload::TaskLedger(record) = event.payload {
                aggregate.apply_record(cursor, &record)?;
            } else {
                aggregate.advance_cursor(cursor);
            }
        }
        if aggregate.task_id() != Some(task_id) {
            return Err(DomainError::LedgerCorrupt);
        }
        aggregate.set_updated_at(updated_at(&self.events_path(task_id))?);
        Ok(aggregate)
    }

    pub fn summaries(&self) -> Result<Vec<TaskSummary>, DomainError> {
        self.repair_dirty_tasks()?;
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
        self.repair_dirty_tasks()?;
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

    fn scan_indexes(
        &self,
    ) -> Result<(RepositoryIndexes, HashMap<TaskId, TaskAggregate>), DomainError> {
        let mut indexes = RepositoryIndexes::default();
        let mut aggregates = HashMap::new();
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
            indexes
                .summaries
                .insert(task_id.clone(), aggregate.summary());
            aggregates.insert(task_id, aggregate);
        }
        Ok((indexes, aggregates))
    }

    fn repair_dirty_tasks(&self) -> Result<(), DomainError> {
        let task_ids: Vec<_> = self
            .state
            .dirty_tasks
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .iter()
            .cloned()
            .collect();
        for task_id in task_ids {
            self.repair_task(&task_id)?;
        }
        Ok(())
    }

    fn repair_task(&self, task_id: &TaskId) -> Result<(), DomainError> {
        let _scan = self
            .state
            .scan_gate
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        let store = kuku::event::EventStore::open(self.events_path(task_id))
            .map_err(|_| DomainError::LedgerCorrupt)?;
        store.with_publication_transaction(|| self.repair_task_exclusive(task_id))
    }

    fn repair_task_exclusive(&self, task_id: &TaskId) -> Result<(), DomainError> {
        let dirty = self
            .state
            .dirty_tasks
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .contains(task_id);
        if self
            .state
            .unconfirmed_tasks
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .contains(task_id)
        {
            self.confirm_durability(task_id)?;
        }
        let repaired_event = if dirty {
            Some(
                self.replay(task_id)?
                    .into_iter()
                    .next_back()
                    .ok_or(DomainError::LedgerCorrupt)?,
            )
        } else {
            None
        };
        #[cfg(test)]
        if let Some((snapshot_reached, resume_repair)) = self
            .state
            .repair_snapshot_pause
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .take()
        {
            snapshot_reached.wait();
            resume_repair.wait();
        }
        let aggregate = self.rebuild(task_id)?;
        self.merge_task_receipts(task_id, None)?;
        self.state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .summaries
            .insert(task_id.clone(), aggregate.summary());
        let projection = aggregate.projection()?;
        self.state
            .aggregates
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .insert(task_id.clone(), aggregate);
        if repaired_event
            .as_ref()
            .is_some_and(|event| event.id != projection.cursor.get())
        {
            return Err(DomainError::LedgerCorrupt);
        }
        self.write_projection(task_id, &projection)?;
        self.clear_dirty(task_id)?;
        if let Some(event) = repaired_event {
            self.notify_observers(&TaskPublication {
                event,
                projection: projection.clone(),
                delta: TaskDelta::ProjectionReplaced {
                    projection: Box::new(projection),
                },
            })?;
        }
        Ok(())
    }

    fn merge_task_receipts(
        &self,
        task_id: &TaskId,
        through: Option<u64>,
    ) -> Result<(), DomainError> {
        let mut receipts = HashMap::new();
        for event in self.replay(task_id)? {
            if through.is_some_and(|cursor| event.id > cursor) {
                break;
            }
            if let Some(transaction) = control_transaction(&event) {
                let key = transaction.command().idempotency_key().to_owned();
                let receipt = DurableReceipt {
                    digest: transaction.command().intent_digest().to_owned(),
                    task_id: task_id.clone(),
                    task_revision: transaction.task_revision(),
                    result: transaction.command().result().clone(),
                };
                if receipts.insert(key, receipt).is_some() {
                    return Err(DomainError::LedgerCorrupt);
                }
            }
        }
        let mut indexes = self
            .state
            .indexes
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?;
        for (key, receipt) in receipts {
            match indexes.receipts.lookup(&key, &receipt.digest)? {
                None => indexes.receipts.insert(key, receipt)?,
                Some(existing) if existing == receipt => {}
                Some(_) => return Err(DomainError::LedgerCorrupt),
            }
        }
        Ok(())
    }

    fn mark_dirty(&self, task_id: &TaskId, event: &StoredEvent) {
        self.state
            .dirty_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(task_id.clone());
        if let Some(transaction) = control_transaction(event) {
            self.state
                .dirty_keys
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    transaction.command().idempotency_key().to_owned(),
                    task_id.clone(),
                );
        }
    }

    fn mark_unconfirmed(&self, task_id: &TaskId, event: &StoredEvent) {
        self.mark_dirty(task_id, event);
        self.state
            .unconfirmed_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(task_id.clone());
    }

    fn clear_dirty(&self, task_id: &TaskId) -> Result<(), DomainError> {
        self.state
            .dirty_tasks
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .remove(task_id);
        self.state
            .dirty_keys
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .retain(|_, value| value != task_id);
        self.state
            .unconfirmed_tasks
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .remove(task_id);
        Ok(())
    }

    fn write_projection(
        &self,
        task_id: &TaskId,
        projection: &TaskProjection,
    ) -> Result<(), DomainError> {
        #[cfg(test)]
        if self.state.fail_next_cache.swap(false, Ordering::SeqCst) {
            return Err(DomainError::LedgerCorrupt);
        }
        let encoded =
            serde_json::to_vec_pretty(projection).map_err(|_| DomainError::LedgerCorrupt)?;
        crate::platform::write_private_atomic(
            &self.task_path(task_id).join("projection.json"),
            &encoded,
        )
        .map_err(|_| DomainError::LedgerCorrupt)
    }

    fn rewrite_projection_caches(&self) -> Result<(), DomainError> {
        let aggregates = self
            .state
            .aggregates
            .lock()
            .map_err(|_| DomainError::LedgerCorrupt)?
            .clone();
        for (task_id, aggregate) in aggregates {
            self.write_projection(&task_id, &aggregate.projection()?)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn fail_next_append_for_test(&self) {
        self.fail_appends_for_test(1);
    }

    #[cfg(test)]
    pub(super) fn fail_appends_for_test(&self, count: usize) {
        self.state.append_failures.store(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(super) fn fail_next_publication_for_test(&self) {
        self.state
            .fail_next_publication
            .store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(super) fn fail_next_cache_for_test(&self) {
        self.state.fail_next_cache.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(super) fn fail_next_store_after_write_for_test(&self) {
        self.state
            .fail_next_store_after_write
            .store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(super) fn fail_durability_confirmations_for_test(&self, count: usize) {
        self.state
            .durability_confirmation_failures
            .store(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(super) fn pause_next_repair_after_snapshot_for_test(
        &self,
        snapshot_reached: Arc<Barrier>,
        resume_repair: Arc<Barrier>,
    ) {
        *self
            .state
            .repair_snapshot_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((snapshot_reached, resume_repair));
    }

    #[cfg(test)]
    pub(super) fn publication_results_len_for_test(&self) -> usize {
        self.state
            .publication_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

fn control_transaction(event: &StoredEvent) -> Option<&kuku::event::TaskTransaction> {
    match &event.payload {
        EventPayload::TaskLedger(TaskLedgerRecord::Control(transaction)) => Some(transaction),
        _ => None,
    }
}

fn delta_has_ui_change(delta: &TaskDelta) -> bool {
    match delta {
        TaskDelta::ProjectionReplaced { .. } => true,
        TaskDelta::ChangesApplied {
            changes,
            timeline_window,
        } => !changes.is_empty() || timeline_window.is_some(),
    }
}
