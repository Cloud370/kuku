use std::collections::HashMap;

use kuku::event::{CommandResult, TaskId, TaskRevision};

use super::domain::DomainError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableReceipt {
    pub digest: String,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub result: CommandResult,
}

#[derive(Debug, Default)]
pub struct IdempotencyIndex {
    entries: HashMap<String, DurableReceipt>,
}

impl IdempotencyIndex {
    pub fn lookup(&self, key: &str, digest: &str) -> Result<Option<DurableReceipt>, DomainError> {
        match self.entries.get(key) {
            None => Ok(None),
            Some(receipt) if receipt.digest == digest => Ok(Some(receipt.clone())),
            Some(_) => Err(DomainError::IdempotencyConflict),
        }
    }

    pub fn insert(&mut self, key: String, receipt: DurableReceipt) -> Result<(), DomainError> {
        if self.entries.insert(key, receipt).is_some() {
            return Err(DomainError::LedgerCorrupt);
        }
        Ok(())
    }
}
