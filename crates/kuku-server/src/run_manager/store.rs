use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use kuku::event::{
    CommandIntent, CommandReceipt, CommandResult, MessageFact, MessageRoleFact,
    ReviewSubmissionRecorded, ReviewSubmissionReference, RunFact, RunId, RunState, TaskEvent,
    TaskId, TaskLedgerRecord, TaskRevision, TaskTransaction,
};

use crate::api::{
    ApiErrorCode, ApiVersion, CommandAccepted, CreateTaskRequest, CreateTaskResponse,
    ListTasksQuery, PageCursor, ReviewSubmissionResult, SubmitRunResponse, TaskPage,
    TaskProjection, TimelinePage, TimelineQuery,
};
use crate::platform::WorkspaceRegistry;

use super::domain::{bounded_timeline_suffix, DomainError, TaskAggregate};
use super::idempotency::DurableReceipt;
use super::repository::TaskRepository;
use super::submission::review_result;
pub use super::submission::{
    ReviewSubmissionValidator, RunQueueAdmission, RunQueueReservation, SkillSelectionValidator,
    SubmitReviewCommand, SubmitRunCommand, ValidatedSkillSelection,
};

pub type CreateTaskCommand = CreateTaskRequest;

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

#[derive(Clone)]
pub struct TaskCommandService {
    repository: TaskRepository,
    workspaces: Option<Arc<WorkspaceRegistry>>,
    skills: Arc<dyn SkillSelectionValidator>,
    reviews: Arc<dyn ReviewSubmissionValidator>,
    queue: Arc<dyn RunQueueAdmission>,
}

impl std::fmt::Debug for TaskCommandService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TaskCommandService")
            .finish_non_exhaustive()
    }
}

impl TaskCommandService {
    pub fn new(
        repository: TaskRepository,
        workspaces: Arc<WorkspaceRegistry>,
        skills: Arc<dyn SkillSelectionValidator>,
        reviews: Arc<dyn ReviewSubmissionValidator>,
        queue: Arc<dyn RunQueueAdmission>,
    ) -> Self {
        Self {
            repository,
            workspaces: Some(workspaces),
            skills,
            reviews,
            queue,
        }
    }

    #[cfg(test)]
    pub(super) fn new_unchecked(repository: TaskRepository) -> Self {
        Self::new_unchecked_with_ports(
            repository,
            Arc::new(super::submission::TestSkillValidator),
            Arc::new(super::submission::TestReviewValidator),
            Arc::new(super::submission::TestQueue),
        )
    }

    #[cfg(test)]
    pub(super) fn new_with_test_ports(
        repository: TaskRepository,
        workspaces: Arc<WorkspaceRegistry>,
    ) -> Self {
        Self::new(
            repository,
            workspaces,
            Arc::new(super::submission::TestSkillValidator),
            Arc::new(super::submission::TestReviewValidator),
            Arc::new(super::submission::TestQueue),
        )
    }

    #[cfg(test)]
    pub(super) fn new_unchecked_with_ports(
        repository: TaskRepository,
        skills: Arc<dyn SkillSelectionValidator>,
        reviews: Arc<dyn ReviewSubmissionValidator>,
        queue: Arc<dyn RunQueueAdmission>,
    ) -> Self {
        Self {
            repository,
            workspaces: None,
            skills,
            reviews,
            queue,
        }
    }

    pub async fn create_task(
        &self,
        command: CreateTaskCommand,
    ) -> Result<CreateTaskResponse, DomainError> {
        let _create = self.repository.create_guard().await;
        let _key = self.repository.key_guard(&command.idempotency_key).await;
        let intent = CommandIntent::CreateTask {
            workspace_id: command.workspace_id.clone(),
        };
        let digest = intent_digest(None, &intent)?;
        if let Some(receipt) = self.repository.receipt(&command.idempotency_key, &digest)? {
            return self.replay_create(receipt);
        }
        let _lease = if let Some(workspaces) = &self.workspaces {
            Some(
                workspaces
                    .lease_for_task(&command.workspace_id)
                    .await
                    .map_err(|error| match error.code() {
                        ApiErrorCode::WorkspaceNotFound => DomainError::WorkspaceNotFound,
                        _ => DomainError::LedgerCorrupt,
                    })?,
            )
        } else {
            None
        };
        let task_id = TaskId::try_new().map_err(|_| DomainError::StorageExhausted)?;
        let receipt = CommandReceipt::new(
            command.idempotency_key,
            digest,
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
                title: "New task".to_owned(),
                created_at: current_timestamp()?,
            }],
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append_initial(&task_id, TaskLedgerRecord::Control(transaction))?;
        let aggregate = self.repository.rebuild(&task_id)?;
        Ok(CreateTaskResponse {
            api_version: ApiVersion,
            projection: aggregate.projection()?,
            replayed: false,
        })
    }

    fn replay_create(&self, receipt: DurableReceipt) -> Result<CreateTaskResponse, DomainError> {
        let CommandResult::TaskCreated { task_id } = receipt.result else {
            return Err(DomainError::LedgerCorrupt);
        };
        if task_id != receipt.task_id {
            return Err(DomainError::LedgerCorrupt);
        }
        Ok(CreateTaskResponse {
            api_version: ApiVersion,
            projection: self.repository.rebuild(&task_id)?.projection()?,
            replayed: true,
        })
    }

    pub async fn projection(&self, task_id: &TaskId) -> Result<TaskProjection, DomainError> {
        self.repository.rebuild(task_id)?.projection()
    }

    pub async fn submit(
        &self,
        command: SubmitRunCommand,
    ) -> Result<SubmitRunResponse, DomainError> {
        let intent = CommandIntent::SubmitMessage {
            message: command.message.clone(),
            tier_id: command.tier_id.clone(),
            skill_ids: command.skill_ids.clone(),
        };
        let digest = intent_digest(Some(&command.task_id), &intent)?;
        let _key = self.repository.key_guard(&command.idempotency_key).await;
        let _task = self.repository.task_guard(&command.task_id).await;
        if let Some(receipt) = self.repository.receipt(&command.idempotency_key, &digest)? {
            return replay_submit(receipt, &command.task_id);
        }
        let aggregate = self.repository.rebuild(&command.task_id)?;
        if aggregate.revision() != command.expected_task_revision {
            return Err(DomainError::StaleCommand);
        }
        if aggregate.projection()?.task.state.is_active() {
            return Err(DomainError::TaskBusy);
        }
        let workspace_id = aggregate
            .workspace_id()
            .ok_or(DomainError::TaskNotCreated)?;
        let validated = self
            .skills
            .validate(workspace_id, &command.tier_id, &command.skill_ids)?;
        let reservation = self.queue.clone().reserve()?;
        let run_id = kuku::event::RunId::try_new().map_err(|_| DomainError::StorageExhausted)?;
        let next_revision = command
            .expected_task_revision
            .checked_next()
            .map_err(|_| DomainError::StorageExhausted)?;
        let receipt = CommandReceipt::new(
            command.idempotency_key.clone(),
            digest.clone(),
            CommandResult::RunSubmitted {
                run_id: run_id.clone(),
            },
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        let started_at = current_timestamp()?;
        let transaction = TaskTransaction::try_new(
            next_revision,
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
                    selection: validated.selection,
                },
                TaskEvent::RunQueued {
                    run: RunFact {
                        run_id: run_id.clone(),
                        task_id: command.task_id.clone(),
                        state: RunState::Queued,
                        started_at,
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
        self.append_submission(
            &command.task_id,
            &command.idempotency_key,
            &digest,
            &run_id,
            transaction,
            reservation,
        )?;
        Ok(SubmitRunResponse {
            api_version: ApiVersion,
            task_id: command.task_id,
            run_id,
            task_revision: next_revision,
            replayed: false,
        })
    }

    pub async fn submit_review(
        &self,
        command: SubmitReviewCommand,
    ) -> Result<ReviewSubmissionResult, DomainError> {
        let intent = CommandIntent::SubmitReview {
            submission_id: command.submission_id.clone(),
        };
        let digest = review_intent_digest(&command, &intent)?;
        let _key = self.repository.key_guard(&command.idempotency_key).await;
        let _task = self.repository.task_guard(&command.task_id).await;
        if let Some(receipt) = self.repository.receipt(&command.idempotency_key, &digest)? {
            return self.replay_review(receipt, &command.task_id);
        }
        let aggregate = self.repository.rebuild(&command.task_id)?;
        if aggregate.revision() != command.expected_task_revision {
            return Err(DomainError::StaleCommand);
        }
        if aggregate.projection()?.task.state.is_active() {
            return Err(DomainError::TaskBusy);
        }
        let notes =
            self.reviews
                .validate(&command.task_id, &command.submission_id, &command.notes)?;
        let reservation = self.queue.clone().reserve()?;
        let run_id = RunId::try_new().map_err(|_| DomainError::StorageExhausted)?;
        let next_revision = command
            .expected_task_revision
            .checked_next()
            .map_err(|_| DomainError::StorageExhausted)?;
        let submitted_at = current_timestamp()?;
        let recorded = ReviewSubmissionRecorded {
            submission_id: command.submission_id.clone(),
            task_id: command.task_id.clone(),
            run_id: run_id.clone(),
            task_revision: next_revision,
            submitted_at: submitted_at.clone(),
            notes,
        };
        let reference = ReviewSubmissionReference {
            submission_id: command.submission_id.clone(),
            task_id: command.task_id.clone(),
            run_id: run_id.clone(),
            task_revision: next_revision,
            submitted_at: submitted_at.clone(),
        };
        let receipt = CommandReceipt::new(
            command.idempotency_key.clone(),
            digest.clone(),
            CommandResult::ReviewSubmitted {
                submission_id: command.submission_id,
            },
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        let transaction = TaskTransaction::try_new(
            next_revision,
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
                TaskEvent::ReviewSubmissionReferenced {
                    submission: reference,
                },
                TaskEvent::ReviewSubmissionRecorded(recorded.clone()),
                TaskEvent::RunQueued {
                    run: RunFact {
                        run_id: run_id.clone(),
                        task_id: command.task_id.clone(),
                        state: RunState::Queued,
                        started_at: submitted_at,
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
        self.append_submission(
            &command.task_id,
            &command.idempotency_key,
            &digest,
            &run_id,
            transaction,
            reservation,
        )?;
        Ok(review_result(recorded, false))
    }

    fn append_submission(
        &self,
        task_id: &TaskId,
        idempotency_key: &str,
        digest: &str,
        run_id: &RunId,
        transaction: TaskTransaction,
        reservation: Box<dyn RunQueueReservation>,
    ) -> Result<(), DomainError> {
        if let Err(error) = self
            .repository
            .append(task_id, TaskLedgerRecord::Control(transaction))
        {
            if self.repository.receipt(idempotency_key, digest)?.is_some() {
                reservation.commit(task_id.clone(), run_id.clone());
            }
            return Err(error);
        }
        reservation.commit(task_id.clone(), run_id.clone());
        Ok(())
    }

    fn replay_review(
        &self,
        receipt: DurableReceipt,
        task_id: &TaskId,
    ) -> Result<ReviewSubmissionResult, DomainError> {
        if &receipt.task_id != task_id {
            return Err(DomainError::LedgerCorrupt);
        }
        let CommandResult::ReviewSubmitted { submission_id } = receipt.result else {
            return Err(DomainError::LedgerCorrupt);
        };
        let recorded = self.repository.review_submission(task_id, &submission_id)?;
        Ok(review_result(recorded, true))
    }

    pub async fn list_tasks(
        &self,
        workspace_id: &kuku::event::WorkspaceId,
        search: Option<&str>,
    ) -> Result<TaskPage, DomainError> {
        self.list_tasks_query(ListTasksQuery {
            workspace_id: workspace_id.clone(),
            search: search.map(str::to_owned),
            cursor: None,
            limit: 100,
        })
        .await
    }

    pub async fn list_tasks_query(&self, query: ListTasksQuery) -> Result<TaskPage, DomainError> {
        if query.limit == 0 || query.limit > 100 {
            return Err(DomainError::InvalidRequest);
        }
        let normalized = normalize_search(query.search.as_deref())?;
        let mut items: Vec<_> = self
            .repository
            .summaries()?
            .into_iter()
            .filter(|summary| summary.workspace_id == query.workspace_id)
            .filter(|summary| {
                normalized
                    .as_ref()
                    .is_none_or(|search| summary.title.to_lowercase().contains(search))
            })
            .collect();
        items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.task_id.cmp(&left.task_id))
        });
        if let Some(cursor) = &query.cursor {
            let cursor = parse_list_cursor(cursor)?;
            if cursor.workspace_id != query.workspace_id
                || cursor.search != normalized
                || cursor.limit != query.limit
            {
                return Err(DomainError::InvalidRequest);
            }
            items.retain(|summary| {
                (&summary.updated_at, &summary.task_id) < (&cursor.updated_at, &cursor.task_id)
            });
        }
        let has_more = items.len() > usize::from(query.limit);
        items.truncate(usize::from(query.limit));
        let next_cursor = if has_more {
            let boundary = items.last().ok_or(DomainError::LedgerCorrupt)?;
            Some(list_cursor(ListCursor {
                workspace_id: query.workspace_id,
                search: normalized,
                limit: query.limit,
                updated_at: boundary.updated_at.clone(),
                task_id: boundary.task_id.clone(),
            })?)
        } else {
            None
        };
        Ok(TaskPage {
            api_version: ApiVersion,
            items,
            next_cursor,
        })
    }

    pub async fn timeline(
        &self,
        task_id: &TaskId,
        query: TimelineQuery,
    ) -> Result<TimelinePage, DomainError> {
        if query.limit == 0 || query.limit > 500 {
            return Err(DomainError::InvalidRequest);
        }
        let (aggregate, end) = if let Some(cursor) = &query.before {
            let cursor = parse_timeline_cursor(cursor)?;
            if cursor.task_id != *task_id || cursor.limit != query.limit {
                return Err(DomainError::InvalidRequest);
            }
            let aggregate = self
                .repository
                .rebuild_at(task_id, cursor.snapshot_cursor)?;
            if cursor.end > aggregate.timeline_items().len() {
                return Err(DomainError::InvalidRequest);
            }
            (aggregate, cursor.end)
        } else {
            let aggregate = self.repository.rebuild(task_id)?;
            let end = aggregate.timeline_items().len();
            (aggregate, end)
        };
        let snapshot_cursor = aggregate.cursor().get();
        let (start, items) = bounded_timeline_suffix(
            aggregate.timeline_items(),
            end,
            usize::from(query.limit),
            |candidate_start| {
                let next_cursor = if candidate_start > 0 {
                    Some(timeline_cursor(TimelineCursor {
                        task_id: task_id.clone(),
                        limit: query.limit,
                        snapshot_cursor,
                        end: candidate_start,
                    })?)
                } else {
                    None
                };
                serde_json::to_vec(&TimelinePage {
                    api_version: ApiVersion,
                    task_id: task_id.clone(),
                    items: Vec::new(),
                    next_cursor,
                })
                .map(|encoded| encoded.len())
                .map_err(|_| DomainError::LedgerCorrupt)
            },
        )?;
        let next_cursor = if start > 0 {
            Some(timeline_cursor(TimelineCursor {
                task_id: task_id.clone(),
                limit: query.limit,
                snapshot_cursor,
                end: start,
            })?)
        } else {
            None
        };
        Ok(TimelinePage {
            api_version: ApiVersion,
            task_id: task_id.clone(),
            items,
            next_cursor,
        })
    }

    pub async fn append_activity(
        &self,
        task_id: &TaskId,
        events: Vec<TaskEvent>,
    ) -> Result<TaskAggregate, DomainError> {
        let _task = self.repository.task_guard(task_id).await;
        let batch = kuku::event::TaskActivityBatch::try_new(events)
            .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(task_id, TaskLedgerRecord::Activity(batch))?;
        self.repository.rebuild(task_id)
    }

    pub async fn stop(&self, command: StopRunCommand) -> Result<CommandAccepted, DomainError> {
        let intent = CommandIntent::Stop;
        let digest = intent_digest(Some(&command.task_id), &intent)?;
        let _key = self.repository.key_guard(&command.idempotency_key).await;
        let _task = self.repository.task_guard(&command.task_id).await;
        if let Some(receipt) = self.repository.receipt(&command.idempotency_key, &digest)? {
            return replay_accepted(receipt, &command.task_id, CommandResult::Stopped);
        }
        let aggregate = self.repository.rebuild(&command.task_id)?;
        if aggregate.revision() != command.expected_task_revision {
            return Err(DomainError::StaleCommand);
        }
        let active = aggregate
            .projection()?
            .active_run
            .ok_or(DomainError::RunNotActive)?;
        let next_revision = command
            .expected_task_revision
            .checked_next()
            .map_err(|_| DomainError::StorageExhausted)?;
        let receipt = CommandReceipt::new(command.idempotency_key, digest, CommandResult::Stopped)
            .map_err(|_| DomainError::LedgerCorrupt)?;
        let record = TaskTransaction::try_new(
            next_revision,
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
        Ok(CommandAccepted {
            api_version: ApiVersion,
            task_id: command.task_id,
            task_revision: next_revision,
            replayed: false,
        })
    }

    pub async fn resolve_interaction(
        &self,
        command: ResolveInteractionCommand,
    ) -> Result<CommandAccepted, DomainError> {
        let intent = CommandIntent::ResolveInteraction {
            interaction_id: command.interaction_id.clone(),
            choice_id: command.choice_id.clone(),
        };
        let digest = intent_digest(Some(&command.task_id), &intent)?;
        let _key = self.repository.key_guard(&command.idempotency_key).await;
        let _task = self.repository.task_guard(&command.task_id).await;
        if let Some(receipt) = self.repository.receipt(&command.idempotency_key, &digest)? {
            return replay_accepted(
                receipt,
                &command.task_id,
                CommandResult::InteractionResolved,
            );
        }
        let aggregate = self.repository.rebuild(&command.task_id)?;
        if aggregate.revision() != command.expected_task_revision {
            return Err(DomainError::StaleCommand);
        }
        if !aggregate.interaction_is_pending(&command.interaction_id) {
            return Err(DomainError::InteractionNotPending);
        }
        let next_revision = command
            .expected_task_revision
            .checked_next()
            .map_err(|_| DomainError::StorageExhausted)?;
        let receipt = CommandReceipt::new(
            command.idempotency_key,
            digest,
            CommandResult::InteractionResolved,
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        let record = TaskTransaction::try_new(
            next_revision,
            receipt,
            vec![TaskEvent::InteractionResolved {
                interaction_id: command.interaction_id,
                choice_id: command.choice_id,
            }],
        )
        .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(&command.task_id, TaskLedgerRecord::Control(record))?;
        Ok(CommandAccepted {
            api_version: ApiVersion,
            task_id: command.task_id,
            task_revision: next_revision,
            replayed: false,
        })
    }

    pub fn repository(&self) -> &TaskRepository {
        &self.repository
    }
}

fn replay_submit(
    receipt: DurableReceipt,
    task_id: &TaskId,
) -> Result<SubmitRunResponse, DomainError> {
    if &receipt.task_id != task_id {
        return Err(DomainError::LedgerCorrupt);
    }
    let CommandResult::RunSubmitted { run_id } = receipt.result else {
        return Err(DomainError::LedgerCorrupt);
    };
    Ok(SubmitRunResponse {
        api_version: ApiVersion,
        task_id: task_id.clone(),
        run_id,
        task_revision: receipt.task_revision,
        replayed: true,
    })
}

fn replay_accepted(
    receipt: DurableReceipt,
    task_id: &TaskId,
    expected: CommandResult,
) -> Result<CommandAccepted, DomainError> {
    if &receipt.task_id != task_id || receipt.result != expected {
        return Err(DomainError::LedgerCorrupt);
    }
    Ok(CommandAccepted {
        api_version: ApiVersion,
        task_id: task_id.clone(),
        task_revision: receipt.task_revision,
        replayed: true,
    })
}

fn intent_digest(task_id: Option<&TaskId>, intent: &CommandIntent) -> Result<String, DomainError> {
    #[derive(Serialize)]
    struct DigestInput<'a> {
        task_id: Option<&'a TaskId>,
        intent: &'a CommandIntent,
    }
    digest(&DigestInput { task_id, intent })
}

fn review_intent_digest(
    command: &SubmitReviewCommand,
    intent: &CommandIntent,
) -> Result<String, DomainError> {
    #[derive(Serialize)]
    struct DigestInput<'a> {
        task_id: &'a TaskId,
        intent: &'a CommandIntent,
        payload_hash: &'a str,
        message: &'a str,
        notes: &'a [kuku::event::ReviewAnnotationFact],
    }
    digest(&DigestInput {
        task_id: &command.task_id,
        intent,
        payload_hash: &command.payload_hash,
        message: &command.message,
        notes: &command.notes,
    })
}

fn digest(value: &impl Serialize) -> Result<String, DomainError> {
    let bytes = serde_json::to_vec(value).map_err(|_| DomainError::LedgerCorrupt)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn current_timestamp() -> Result<String, DomainError> {
    super::domain::system_time_rfc3339(std::time::SystemTime::now())
}

fn normalize_search(search: Option<&str>) -> Result<Option<String>, DomainError> {
    let search = search.map(str::trim).filter(|value| !value.is_empty());
    if search.is_some_and(|value| value.len() > 256) {
        return Err(DomainError::InvalidRequest);
    }
    Ok(search.map(str::to_lowercase))
}

#[derive(Serialize, Deserialize)]
struct ListCursor {
    workspace_id: kuku::event::WorkspaceId,
    search: Option<String>,
    limit: u16,
    updated_at: String,
    task_id: TaskId,
}

fn list_cursor(cursor: ListCursor) -> Result<PageCursor, DomainError> {
    let value = serde_json::to_string(&cursor).map_err(|_| DomainError::LedgerCorrupt)?;
    PageCursor::try_new(format!("task-list:v2:{value}")).map_err(|_| DomainError::LedgerCorrupt)
}

fn parse_list_cursor(cursor: &PageCursor) -> Result<ListCursor, DomainError> {
    let value = cursor
        .as_str()
        .strip_prefix("task-list:v2:")
        .ok_or(DomainError::InvalidRequest)?;
    serde_json::from_str(value).map_err(|_| DomainError::InvalidRequest)
}

#[derive(Serialize, Deserialize)]
pub(super) struct TimelineCursor {
    pub task_id: TaskId,
    pub limit: u16,
    pub snapshot_cursor: u64,
    pub end: usize,
}

pub(super) fn timeline_cursor(cursor: TimelineCursor) -> Result<PageCursor, DomainError> {
    let value = serde_json::to_string(&cursor).map_err(|_| DomainError::LedgerCorrupt)?;
    PageCursor::try_new(format!("timeline:v2:{value}")).map_err(|_| DomainError::LedgerCorrupt)
}

fn parse_timeline_cursor(cursor: &PageCursor) -> Result<TimelineCursor, DomainError> {
    let value = cursor
        .as_str()
        .strip_prefix("timeline:v2:")
        .ok_or(DomainError::InvalidRequest)?;
    serde_json::from_str(value).map_err(|_| DomainError::InvalidRequest)
}
