use crate::error::Error;
use crate::event::{
    Cursor, EventPayload, EventStore, StorageExhaustionError, TaskActivityBatch, TaskEvent,
    TaskLedgerError, TaskLedgerRecord,
};

/// Failure to append request evidence to the Task ledger.
#[derive(Debug, thiserror::Error)]
pub enum ContextFactSinkError {
    /// The requested events do not form a valid activity batch.
    #[error("context activity batch is invalid")]
    InvalidBatch(#[source] TaskLedgerError),
    /// The durable append or ledger permission update failed.
    #[error("context activity append failed")]
    Append(#[source] Error),
    /// The stored event ID cannot be represented by a wire cursor.
    #[error("context activity cursor is invalid")]
    InvalidCursor(#[source] StorageExhaustionError),
    /// Snapshot and start values disagree on request identity or configuration.
    #[error("request snapshot and lifecycle start do not describe the same request")]
    MismatchedRequestEvidence,
    /// The active query recorder cannot persist exact request evidence.
    #[error("query recorder cannot persist exact request evidence")]
    RecorderUnavailable,
}

/// Durable sink for Context-owned Task activity facts.
pub trait ContextFactSink: std::fmt::Debug + Send + Sync {
    /// Atomically appends a checked Task activity batch.
    fn append_activity(&self, events: Vec<TaskEvent>) -> Result<Cursor, ContextFactSinkError>;
}

/// Context fact sink backed by the canonical Task event store.
#[derive(Debug, Clone)]
pub struct EventStoreContextFactSink {
    event_store: EventStore,
}

impl EventStoreContextFactSink {
    /// Protects the ledger and creates an adapter over it.
    pub fn new(event_store: EventStore) -> Result<Self, ContextFactSinkError> {
        protect_ledger(&event_store)?;
        Ok(Self { event_store })
    }
}

impl ContextFactSink for EventStoreContextFactSink {
    fn append_activity(&self, events: Vec<TaskEvent>) -> Result<Cursor, ContextFactSinkError> {
        let batch =
            TaskActivityBatch::try_new(events).map_err(ContextFactSinkError::InvalidBatch)?;
        let mut event_store = self.event_store.clone();
        let stored = event_store
            .append_synced(EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)))
            .map_err(ContextFactSinkError::Append)?;
        Cursor::try_new(stored.id).map_err(ContextFactSinkError::InvalidCursor)
    }
}

#[cfg(unix)]
fn protect_ledger(event_store: &EventStore) -> Result<(), ContextFactSinkError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(event_store.path(), std::fs::Permissions::from_mode(0o600))
        .map_err(Error::from)
        .map_err(ContextFactSinkError::Append)
}

#[cfg(not(unix))]
fn protect_ledger(_event_store: &EventStore) -> Result<(), ContextFactSinkError> {
    Ok(())
}
