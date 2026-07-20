use std::sync::Arc;

use tokio::sync::Mutex;

use kuku::event::{
    CommandReceipt, CommandResult, TaskEvent, TaskId, TaskLedgerRecord, TaskRevision,
    TaskTransaction, WorkspaceId,
};

use super::domain::{DomainError, TaskAggregate};
use super::idempotency::IdempotencyIndex;
use super::repository::TaskRepository;

#[derive(Debug, Clone)]
pub struct CreateTaskCommand {
    pub workspace_id: WorkspaceId,
    pub idempotency_key: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct TaskCommandService {
    repository: TaskRepository,
    gate: Arc<Mutex<()>>,
    idempotency: Arc<Mutex<IdempotencyIndex>>,
}

impl TaskCommandService {
    pub fn new(repository: TaskRepository) -> Self {
        Self {
            repository,
            gate: Arc::new(Mutex::new(())),
            idempotency: Arc::new(Mutex::new(IdempotencyIndex::default())),
        }
    }

    pub async fn create_task(
        &self,
        command: CreateTaskCommand,
    ) -> Result<TaskAggregate, DomainError> {
        let _gate = self.gate.lock().await;
        let digest = format!("{}:{}", command.workspace_id, command.title);
        if let Some(task_id) = self
            .idempotency
            .lock()
            .await
            .lookup(&command.idempotency_key, &digest)
            .map_err(|_| DomainError::LedgerCorrupt)?
        {
            return self.repository.rebuild(&task_id);
        }
        let task_id = TaskId::try_new().map_err(|_| DomainError::StorageExhausted)?;
        let receipt = CommandReceipt::new(
            command.idempotency_key.clone(),
            digest.clone(),
            CommandResult::TaskCreated {
                task_id: task_id.clone(),
            },
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        let transaction = TaskTransaction::try_new(
            TaskRevision::try_new(0).map_err(|_| DomainError::StorageExhausted)?,
            receipt,
            vec![TaskEvent::TaskCreated {
                task_id: task_id.clone(),
                workspace_id: command.workspace_id,
                title: command.title,
                created_at: "1970-01-01T00:00:00Z".to_string(),
            }],
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(&task_id, TaskLedgerRecord::Control(transaction))?;
        self.idempotency
            .lock()
            .await
            .insert(command.idempotency_key, digest, task_id.clone());
        self.repository.rebuild(&task_id)
    }

    pub async fn projection(
        &self,
        task_id: &TaskId,
    ) -> Result<crate::api::TaskProjection, DomainError> {
        self.repository.rebuild(task_id)?.projection()
    }

    pub fn repository(&self) -> &TaskRepository {
        &self.repository
    }
}
