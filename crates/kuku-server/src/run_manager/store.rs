use std::sync::Arc;

use tokio::sync::Mutex;

use kuku::event::{
    CommandReceipt, CommandResult, MessageFact, MessageRoleFact, RunFact, RunState, TaskEvent,
    TaskId, TaskLedgerRecord, TaskRevision, TaskTransaction, WorkspaceId,
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
pub struct SubmitRunCommand {
    pub task_id: TaskId,
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
    pub message: String,
    pub tier_id: String,
    pub skill_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StopRunCommand {
    pub task_id: TaskId,
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct ResolveInteractionCommand {
    pub task_id: TaskId,
    pub interaction_id: kuku::event::InteractionId,
    pub choice_id: String,
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct TaskCommandService {
    repository: TaskRepository,
    gate: Arc<Mutex<()>>,
    idempotency: Arc<Mutex<IdempotencyIndex>>,
}

impl TaskCommandService {
    pub fn new(repository: TaskRepository) -> Self {
        let mut idempotency = IdempotencyIndex::default();
        if let Ok(task_ids) = repository.task_ids() {
            for task_id in task_ids {
                if let Ok(events) = repository.replay(&task_id) {
                    for event in events {
                        if let kuku::event::EventPayload::TaskLedger(TaskLedgerRecord::Control(transaction)) = event.payload {
                            idempotency.insert(
                                transaction.command().idempotency_key().to_owned(),
                                transaction.command().intent_digest().to_owned(),
                                task_id.clone(),
                            );
                        }
                    }
                }
            }
        }
        Self {
            repository,
            gate: Arc::new(Mutex::new(())),
            idempotency: Arc::new(Mutex::new(idempotency)),
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

    pub async fn submit(&self, command: SubmitRunCommand) -> Result<TaskAggregate, DomainError> {
        let _gate = self.gate.lock().await;
        let aggregate = self.repository.rebuild(&command.task_id)?;
        let digest = format!(
            "{}:{}:{}:{:?}",
            command.task_id, command.message, command.tier_id, command.skill_ids
        );
        if let Some(task_id) = self
            .idempotency
            .lock()
            .await
            .lookup(&command.idempotency_key, &digest)
            .map_err(|_| DomainError::IdempotencyConflict)?
        {
            return self.repository.rebuild(&task_id);
        }
        if aggregate.revision() != command.expected_task_revision {
            return Err(DomainError::StaleCommand);
        }
        if aggregate.projection()?.task.state.is_active() {
            return Err(DomainError::TaskBusy);
        }
        let run_id = kuku::event::RunId::try_new().map_err(|_| DomainError::StorageExhausted)?;
        let receipt = CommandReceipt::new(
            command.idempotency_key.clone(),
            digest.clone(),
            CommandResult::RunSubmitted {
                run_id: run_id.clone(),
            },
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        let transaction = TaskTransaction::try_new(
            command
                .expected_task_revision
                .checked_next()
                .map_err(|_| DomainError::StorageExhausted)?,
            receipt,
            vec![
                TaskEvent::MessageAppended {
                    message: MessageFact {
                        message_id: format!("msg_{}", run_id.as_str()),
                        task_id: command.task_id.clone(),
                        run_id: Some(run_id.clone()),
                        role: MessageRoleFact::User,
                        text: command.message,
                        finalized: true,
                        request_ids: Vec::new(),
                        file_references: Vec::new(),
                    },
                },
                TaskEvent::SkillsChanged {
                    selection: kuku::event::SkillsChangedFact {
                        tier_id: command.tier_id,
                        skill_ids: command.skill_ids,
                    },
                },
                TaskEvent::RunQueued {
                    run: RunFact {
                        run_id,
                        task_id: command.task_id.clone(),
                        state: RunState::Queued,
                        started_at: "1970-01-01T00:00:00Z".into(),
                        finished_at: None,
                        summary: None,
                        checks: None,
                        metrics: None,
                        workspace_changes: None,
                    },
                },
            ],
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(&command.task_id, TaskLedgerRecord::Control(transaction))?;
        self.idempotency.lock().await.insert(
            command.idempotency_key,
            digest,
            command.task_id.clone(),
        );
        self.repository.rebuild(&command.task_id)
    }

    pub async fn list_tasks(
        &self,
        workspace_id: &WorkspaceId,
        search: Option<&str>,
    ) -> Result<crate::api::TaskPage, DomainError> {
        let search = search
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty());
        let mut items = Vec::new();
        for task_id in self.repository.task_ids()? {
            let summary = self.repository.rebuild(&task_id)?.summary();
            if &summary.workspace_id != workspace_id {
                continue;
            }
            if search
                .as_ref()
                .is_some_and(|query| !summary.title.to_lowercase().contains(query))
            {
                continue;
            }
            items.push(summary);
        }
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.task_id.cmp(&left.task_id))
        });
        Ok(crate::api::TaskPage {
            api_version: crate::api::ApiVersion,
            items,
            next_cursor: None,
        })
    }

    pub async fn list_tasks_query(
        &self,
        query: crate::api::ListTasksQuery,
    ) -> Result<crate::api::TaskPage, DomainError> {
        if query.limit == 0 || query.limit > 100 {
            return Err(DomainError::InvalidRequest);
        }
        let mut page = self.list_tasks(&query.workspace_id, query.search.as_deref()).await?;
        let normalized = query.search.clone().unwrap_or_default().trim().to_lowercase();
        if let Some(cursor) = &query.cursor {
            let prefix = format!("task-list:v1:{}:{}:{}:", query.workspace_id, normalized, query.limit);
            let boundary = cursor.as_str().strip_prefix(&prefix).ok_or(DomainError::InvalidRequest)?;
            let boundary = TaskId::parse(boundary).map_err(|_| DomainError::InvalidRequest)?;
            let position = page.items.iter().position(|item| item.task_id == boundary).ok_or(DomainError::InvalidRequest)?;
            page.items.drain(..=position);
        }
        let has_more = page.items.len() > query.limit as usize;
        page.items.truncate(query.limit as usize);
        if has_more {
            page.next_cursor = crate::api::PageCursor::try_new(format!(
                "task-list:v1:{}:{}:{}:{}",
                query.workspace_id,
                normalized,
                query.limit,
                page.items.last().map(|item| item.task_id.as_str()).unwrap_or_default()
            )).ok();
        }
        Ok(page)
    }

    pub async fn timeline(
        &self,
        task_id: &TaskId,
        query: crate::api::TimelineQuery,
    ) -> Result<crate::api::TimelinePage, DomainError> {
        if query.limit == 0 || query.limit > 500 {
            return Err(DomainError::InvalidRequest);
        }
        let aggregate = self.repository.rebuild(task_id)?;
        let all = aggregate.timeline_items();
        let (snapshot_len, end) = if let Some(cursor) = &query.before {
            let prefix = format!("timeline:{}:{}:", task_id, query.limit);
            let suffix = cursor.as_str().strip_prefix(&prefix).ok_or(DomainError::InvalidRequest)?;
            let (snapshot, end) = suffix.split_once(':').ok_or(DomainError::InvalidRequest)?;
            let snapshot = snapshot.parse::<usize>().map_err(|_| DomainError::InvalidRequest)?;
            let end = end.parse::<usize>().map_err(|_| DomainError::InvalidRequest)?;
            if end > snapshot || snapshot > all.len() { return Err(DomainError::InvalidRequest); }
            (snapshot, end)
        } else {
            (all.len(), all.len())
        };
        let start = end.saturating_sub(query.limit as usize);
        let items = all[start..end].to_vec();
        Ok(crate::api::TimelinePage {
            api_version: crate::api::ApiVersion,
            task_id: task_id.clone(),
            items,
            next_cursor: if start > 0 {
                crate::api::PageCursor::try_new(format!("timeline:{}:{}:{}:{}", task_id, query.limit, snapshot_len, start)).ok()
            } else { None },
        })
    }

    pub async fn append_activity(
        &self,
        task_id: &TaskId,
        events: Vec<TaskEvent>,
    ) -> Result<TaskAggregate, DomainError> {
        let _gate = self.gate.lock().await;
        let batch = kuku::event::TaskActivityBatch::try_new(events)
            .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(task_id, TaskLedgerRecord::Activity(batch))?;
        self.repository.rebuild(task_id)
    }

    pub async fn stop(&self, command: StopRunCommand) -> Result<TaskAggregate, DomainError> {
        let _gate = self.gate.lock().await;
        let aggregate = self.repository.rebuild(&command.task_id)?;
        if aggregate.revision() != command.expected_task_revision {
            return Err(DomainError::StaleCommand);
        }
        let active = aggregate
            .projection()?
            .active_run
            .ok_or(DomainError::TaskNotFound)?;
        let receipt = CommandReceipt::new(
            command.idempotency_key,
            format!("stop:{}", command.task_id),
            CommandResult::Stopped,
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        let record = TaskTransaction::try_new(
            command
                .expected_task_revision
                .checked_next()
                .map_err(|_| DomainError::StorageExhausted)?,
            receipt,
            vec![TaskEvent::RunStopping {
                run: RunFact {
                    run_id: active.run_id,
                    task_id: command.task_id.clone(),
                    state: RunState::Stopping,
                    started_at: active.started_at,
                    finished_at: None,
                    summary: None,
                    checks: None,
                    metrics: None,
                    workspace_changes: None,
                },
            }],
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(&command.task_id, TaskLedgerRecord::Control(record))?;
        self.repository.rebuild(&command.task_id)
    }

    pub async fn resolve_interaction(
        &self,
        command: ResolveInteractionCommand,
    ) -> Result<TaskAggregate, DomainError> {
        let _gate = self.gate.lock().await;
        let aggregate = self.repository.rebuild(&command.task_id)?;
        if aggregate.revision() != command.expected_task_revision {
            return Err(DomainError::StaleCommand);
        }
        let receipt = CommandReceipt::new(
            command.idempotency_key,
            format!("resolve:{}:{}", command.interaction_id, command.choice_id),
            CommandResult::InteractionResolved,
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        let record = TaskTransaction::try_new(
            command
                .expected_task_revision
                .checked_next()
                .map_err(|_| DomainError::StorageExhausted)?,
            receipt,
            vec![TaskEvent::InteractionResolved {
                interaction_id: command.interaction_id,
                choice_id: command.choice_id,
            }],
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(&command.task_id, TaskLedgerRecord::Control(record))?;
        self.repository.rebuild(&command.task_id)
    }

    pub fn repository(&self) -> &TaskRepository {
        &self.repository
    }
}
