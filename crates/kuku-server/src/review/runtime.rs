//! Adapts the durable Task runtime to the Review runtime port.

use std::sync::Arc;

use kuku::event::{
    EventPayload, ReviewAnnotationFact, ReviewSubmissionRecorded, TaskEvent, TaskLedgerRecord,
};
use sha2::{Digest, Sha256};

use crate::api::{
    AnnotationStatus, ApiError, ApiErrorCode, ApiVersion, PageCursor, ReviewSubmissionPage,
    ReviewSubmissionProjection, ReviewSubmissionResult, SubmittedReviewNote, TaskId,
};
use crate::run_manager::{SubmitReviewCommand, TaskRepository, TaskRuntime};

use super::annotations::payload_hash_for_notes;
use super::{
    ReplayLookup, ReviewFuture, ReviewLimits, ReviewRuntimePort, TaskReviewContext,
    ValidatedReviewCommand,
};

const CURSOR_DOMAIN: &str = "review-submissions-v1";
const TRACE_ID: &str = "review-runtime";

/// Connects Review validation to the durable Task runtime and ledger.
#[derive(Clone)]
pub struct RuntimeReviewPort {
    runtime: Arc<TaskRuntime>,
    repository: TaskRepository,
    limits: ReviewLimits,
}

impl RuntimeReviewPort {
    /// Creates an adapter over a runtime and its shared durable repository.
    pub fn new(runtime: Arc<TaskRuntime>, repository: TaskRepository) -> Self {
        Self::with_limits(runtime, repository, ReviewLimits::default())
    }

    /// Creates an adapter with explicit replay and projection limits.
    pub fn with_limits(
        runtime: Arc<TaskRuntime>,
        repository: TaskRepository,
        limits: ReviewLimits,
    ) -> Self {
        Self {
            runtime,
            repository,
            limits,
        }
    }
}

impl ReviewRuntimePort for RuntimeReviewPort {
    fn task_context<'a>(&'a self, task_id: &'a TaskId) -> ReviewFuture<'a, TaskReviewContext> {
        Box::pin(async move {
            let projection = self
                .runtime
                .projection(task_id)
                .await
                .map_err(|error| error.into_api_error(TRACE_ID))?;
            Ok(TaskReviewContext {
                task_id: task_id.clone(),
                workspace_id: projection.task.workspace_id,
                task_revision: projection.task_revision,
                active_run: projection.active_run.is_some(),
            })
        })
    }

    fn lookup_submission<'a>(
        &'a self,
        task_id: &'a TaskId,
        idempotency_key: &'a str,
        payload_hash: &'a str,
    ) -> ReviewFuture<'a, ReplayLookup> {
        Box::pin(async move {
            let events = self
                .repository
                .replay(task_id)
                .map_err(|error| error.into_api_error(TRACE_ID))?;
            let mut scanned = 0u32;
            for event in events {
                let EventPayload::TaskLedger(TaskLedgerRecord::Control(transaction)) =
                    event.payload
                else {
                    continue;
                };
                scanned = scanned.saturating_add(transaction.events().len() as u32);
                if scanned > self.limits.revision_listing_entries {
                    return Err(payload_too_large());
                }
                if transaction.command().idempotency_key() != idempotency_key {
                    continue;
                }
                let Some(recorded) = transaction.events().iter().find_map(recorded_review) else {
                    return Ok(ReplayLookup::Conflict);
                };
                let notes = recorded
                    .notes
                    .iter()
                    .map(annotation_draft)
                    .collect::<Vec<_>>();
                return if payload_hash_for_notes(&notes)? == payload_hash {
                    Ok(ReplayLookup::Replay(review_result(recorded, true)))
                } else {
                    Ok(ReplayLookup::Conflict)
                };
            }
            Ok(ReplayLookup::Missing)
        })
    }

    fn submit_validated<'a>(
        &'a self,
        command: ValidatedReviewCommand,
    ) -> ReviewFuture<'a, ReviewSubmissionResult> {
        Box::pin(async move {
            let submission_id =
                deterministic_submission_id(&command.idempotency_key, &command.payload_hash)?;
            let notes = command
                .notes
                .iter()
                .map(|note| ReviewAnnotationFact {
                    path: note.path.clone(),
                    revision: note.revision.clone(),
                    side: note.side,
                    start_line: note.start_line,
                    end_line: note.end_line,
                    excerpt: note.excerpt.clone(),
                    comment: note.comment.clone(),
                })
                .collect::<Vec<_>>();
            let message = command
                .notes
                .iter()
                .map(|note| note.comment.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            self.runtime
                .submit_review(SubmitReviewCommand {
                    task_id: command.task_id,
                    expected_task_revision: command.expected_task_revision,
                    idempotency_key: command.idempotency_key,
                    submission_id,
                    payload_hash: command.payload_hash,
                    message,
                    notes,
                })
                .await
                .map_err(|error| error.into_api_error(TRACE_ID))
        })
    }

    fn list_submissions<'a>(
        &'a self,
        task_id: &'a TaskId,
        cursor: Option<&'a PageCursor>,
        limit: u16,
    ) -> ReviewFuture<'a, ReviewSubmissionPage> {
        Box::pin(async move {
            let offset = parse_cursor(cursor, task_id)?;
            let mut records = Vec::new();
            for event in self
                .repository
                .replay(task_id)
                .map_err(|error| error.into_api_error(TRACE_ID))?
            {
                let EventPayload::TaskLedger(TaskLedgerRecord::Control(transaction)) =
                    event.payload
                else {
                    continue;
                };
                records.extend(transaction.events().iter().filter_map(recorded_review));
                if records.len() > self.limits.revision_listing_entries as usize {
                    return Err(payload_too_large());
                }
            }
            if offset > records.len() {
                return Err(invalid_cursor());
            }
            let end = offset.saturating_add(limit as usize).min(records.len());
            let items = records[offset..end]
                .iter()
                .cloned()
                .map(review_projection)
                .collect::<Vec<_>>();
            let next_cursor = (end < records.len())
                .then(|| {
                    PageCursor::try_new(format!("{CURSOR_DOMAIN}:{}:{end}", task_id.as_str()))
                        .map_err(|_| internal("review cursor cannot be encoded"))
                })
                .transpose()?;
            Ok(ReviewSubmissionPage {
                api_version: ApiVersion,
                task_id: task_id.clone(),
                items,
                next_cursor,
            })
        })
    }
}

fn recorded_review(event: &TaskEvent) -> Option<ReviewSubmissionRecorded> {
    if let TaskEvent::ReviewSubmissionRecorded(recorded) = event {
        Some(recorded.clone())
    } else {
        None
    }
}

fn annotation_draft(note: &ReviewAnnotationFact) -> crate::api::AnnotationDraft {
    crate::api::AnnotationDraft {
        path: note.path.clone(),
        revision: note.revision.clone(),
        side: note.side,
        start_line: note.start_line,
        end_line: note.end_line,
        excerpt: note.excerpt.clone(),
        comment: note.comment.clone(),
    }
}

fn review_result(recorded: ReviewSubmissionRecorded, replayed: bool) -> ReviewSubmissionResult {
    ReviewSubmissionResult {
        api_version: ApiVersion,
        submission: review_projection(recorded),
        replayed,
    }
}

fn review_projection(recorded: ReviewSubmissionRecorded) -> ReviewSubmissionProjection {
    ReviewSubmissionProjection {
        submission_id: recorded.submission_id,
        task_id: recorded.task_id,
        run_id: recorded.run_id,
        task_revision: recorded.task_revision,
        submitted_at: recorded.submitted_at,
        notes: recorded
            .notes
            .into_iter()
            .map(|note| SubmittedReviewNote {
                path: note.path,
                revision: note.revision,
                side: note.side,
                start_line: note.start_line,
                end_line: note.end_line,
                excerpt: note.excerpt,
                comment: note.comment,
                status: AnnotationStatus::Current,
            })
            .collect(),
    }
}

fn parse_cursor(cursor: Option<&PageCursor>, task_id: &TaskId) -> Result<usize, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let fields = cursor.as_str().split(':').collect::<Vec<_>>();
    if fields.len() != 3 || fields[0] != CURSOR_DOMAIN || fields[1] != task_id.as_str() {
        return Err(invalid_cursor());
    }
    fields[2].parse().map_err(|_| invalid_cursor())
}

fn invalid_cursor() -> ApiError {
    ApiError::new(
        ApiErrorCode::InvalidRequest,
        "review cursor does not match task",
        TRACE_ID,
    )
}

fn internal(message: &str) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, message, TRACE_ID)
}

fn deterministic_submission_id(
    idempotency_key: &str,
    payload_hash: &str,
) -> Result<kuku::event::ReviewSubmissionId, ApiError> {
    let mut digest = Sha256::new();
    digest.update(idempotency_key.as_bytes());
    digest.update([0]);
    digest.update(payload_hash.as_bytes());
    let suffix = format!("{:x}", digest.finalize());
    kuku::event::ReviewSubmissionId::parse(format!("rsub_{}", &suffix[..24])).map_err(|_| {
        ApiError::new(
            ApiErrorCode::StorageExhausted,
            "review submission id is unavailable",
            TRACE_ID,
        )
    })
}

fn payload_too_large() -> ApiError {
    ApiError::new(
        ApiErrorCode::PayloadTooLarge,
        "review ledger projection is too large",
        TRACE_ID,
    )
}

#[cfg(test)]
mod tests {
    use super::deterministic_submission_id;

    #[test]
    fn submission_id_is_stable_for_the_same_idempotency_payload_pair() {
        let first = deterministic_submission_id("key", "hash").unwrap();
        let second = deterministic_submission_id("key", "hash").unwrap();
        let different = deterministic_submission_id("other", "hash").unwrap();
        assert_eq!(first, second);
        assert_ne!(first, different);
        assert!(first.as_str().starts_with("rsub_"));
    }
}
