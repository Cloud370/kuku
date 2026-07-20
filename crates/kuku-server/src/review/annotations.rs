//! Validates revision-anchored annotations before one runtime transaction.

use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api::{
    AnnotationBatch, AnnotationSide, ApiError, ApiErrorCode, ReviewSubmissionResult, TaskId,
};

use super::files::WorkspaceReadService;
use super::git::GitReviewService;
use super::{
    ReplayLookup, ReviewAdmission, ReviewLimits, ReviewRuntimePort, TaskReviewContext,
    ValidatedAnnotation, ValidatedReviewCommand, WorkspaceCapabilityProvider,
};

const TRACE_ID: &str = "review-annotations";

/// Validates all annotation anchors and delegates one atomic runtime command.
pub struct AnnotationService {
    workspaces: Arc<dyn WorkspaceCapabilityProvider>,
    runtime: Arc<dyn ReviewRuntimePort>,
    files: Arc<WorkspaceReadService>,
    admission: Arc<ReviewAdmission>,
    limits: ReviewLimits,
}

impl AnnotationService {
    /// Creates an annotation service using the supplied workspace and runtime ports.
    pub fn new(
        workspaces: Arc<dyn WorkspaceCapabilityProvider>,
        runtime: Arc<dyn ReviewRuntimePort>,
        limits: ReviewLimits,
    ) -> Self {
        let admission = Arc::new(ReviewAdmission::new(&limits));
        let files = Arc::new(WorkspaceReadService::with_admission(
            workspaces.clone(),
            limits.clone(),
            admission.clone(),
        ));
        Self::with_services(workspaces, runtime, files, admission, limits)
    }

    /// Creates an annotation service using shared file and admission services.
    pub fn with_services(
        workspaces: Arc<dyn WorkspaceCapabilityProvider>,
        runtime: Arc<dyn ReviewRuntimePort>,
        files: Arc<WorkspaceReadService>,
        admission: Arc<ReviewAdmission>,
        limits: ReviewLimits,
    ) -> Self {
        Self {
            workspaces,
            runtime,
            files,
            admission,
            limits,
        }
    }

    /// Validates and atomically submits one annotation batch.
    pub async fn submit(
        &self,
        task_id: &TaskId,
        batch: AnnotationBatch,
    ) -> Result<ReviewSubmissionResult, ApiError> {
        let (notes, payload_hash) = normalize_batch(&batch, &self.limits)?;
        match self
            .runtime
            .lookup_submission(task_id, &batch.idempotency_key, &payload_hash)
            .await?
        {
            ReplayLookup::Missing => {}
            ReplayLookup::Replay(mut result) => {
                result.replayed = true;
                return Ok(result);
            }
            ReplayLookup::Conflict => return Err(idempotency_conflict()),
        }
        let context = self.runtime.task_context(task_id).await?;
        validate_context(task_id, &context, &batch)?;
        let capability = self.workspaces.capability(&context.workspace_id)?;
        let git = GitReviewService::with_admission(
            capability,
            self.limits.clone(),
            self.admission.clone(),
        );
        let mut validated = Vec::with_capacity(notes.len());
        for note in notes {
            let excerpt = match note.side {
                AnnotationSide::File => {
                    let content = self
                        .files
                        .content(
                            &context.workspace_id,
                            &note.path,
                            note.start_line,
                            note.end_line,
                        )
                        .await?;
                    if content.revision != note.revision || content.binary {
                        return Err(outdated());
                    }
                    content.text.ok_or_else(outdated)?
                }
                AnnotationSide::Old | AnnotationSide::New => {
                    self.diff_excerpt(
                        &git,
                        &note.path,
                        &note.revision,
                        note.side,
                        note.start_line,
                        note.end_line,
                    )
                    .await?
                }
            };
            if excerpt != note.excerpt {
                return Err(outdated());
            }
            validated.push(ValidatedAnnotation {
                path: note.path,
                revision: note.revision,
                side: note.side,
                start_line: note.start_line,
                end_line: note.end_line,
                excerpt,
                comment: note.comment,
            });
        }
        for note in &validated {
            let current = match note.side {
                AnnotationSide::File => {
                    self.files
                        .current_revision(&context.workspace_id, &note.path)
                        .await?
                        == note.revision
                }
                AnnotationSide::Old | AnnotationSide::New => {
                    self.current_git_revision(&git, &note.path).await?
                        == Some(note.revision.clone())
                }
            };
            if !current {
                return Err(outdated());
            }
        }
        let command = ValidatedReviewCommand {
            task_id: task_id.clone(),
            workspace_id: context.workspace_id,
            expected_task_revision: batch.expected_task_revision,
            idempotency_key: batch.idempotency_key,
            payload_hash,
            notes: validated,
        };
        self.runtime.submit_validated(command).await
    }

    async fn diff_excerpt(
        &self,
        git: &GitReviewService,
        path: &str,
        revision: &crate::api::RevisionToken,
        side: AnnotationSide,
        start: u32,
        end: u32,
    ) -> Result<String, ApiError> {
        let mut cursor = None;
        let mut lines = Vec::with_capacity((end - start + 1) as usize);
        loop {
            let diff = git
                .diff(path, revision, cursor.as_ref(), self.page_limit())
                .await?;
            lines.extend(diff_lines(&diff, side, start, end));
            if lines.len() == (end - start + 1) as usize {
                return Ok(lines.join("\n"));
            }
            cursor = diff.next_cursor;
            if cursor.is_none() {
                return Err(outdated());
            }
        }
    }

    async fn current_git_revision(
        &self,
        git: &GitReviewService,
        path: &str,
    ) -> Result<Option<crate::api::RevisionToken>, ApiError> {
        let mut cursor = None;
        loop {
            let snapshot = git.snapshot(cursor.as_ref(), self.page_limit()).await?;
            if let Some(entry) = snapshot
                .entries
                .into_iter()
                .find(|entry| entry.path == path)
            {
                return Ok(Some(entry.revision));
            }
            cursor = snapshot.next_cursor;
            if cursor.is_none() {
                return Ok(None);
            }
        }
    }

    fn page_limit(&self) -> u16 {
        self.limits.diff_lines.clamp(1, u16::MAX as u32) as u16
    }
}

#[derive(Debug, Clone, Serialize)]
struct NormalizedNote<'a> {
    path: &'a str,
    revision: &'a crate::api::RevisionToken,
    side: AnnotationSide,
    start_line: u32,
    end_line: u32,
    excerpt: &'a str,
    comment: &'a str,
}

fn normalize_batch(
    batch: &AnnotationBatch,
    limits: &ReviewLimits,
) -> Result<(Vec<crate::api::AnnotationDraft>, String), ApiError> {
    if batch.notes.is_empty() {
        return Err(invalid("annotation batch must contain at least one note"));
    }
    if batch.notes.len() > limits.notes_per_batch as usize {
        return Err(payload_too_large());
    }
    if batch.idempotency_key.trim().is_empty() {
        return Err(invalid("annotation idempotency key is required"));
    }
    let mut normalized = Vec::with_capacity(batch.notes.len());
    for note in &batch.notes {
        if note.path.is_empty()
            || note.start_line == 0
            || note.end_line < note.start_line
            || note.comment.len() > limits.comment_bytes
            || note.excerpt.len() > limits.excerpt_bytes
        {
            return Err(
                if note.comment.len() > limits.comment_bytes
                    || note.excerpt.len() > limits.excerpt_bytes
                {
                    payload_too_large()
                } else {
                    invalid("annotation range or path is invalid")
                },
            );
        }
        if normalized
            .iter()
            .any(|existing: &crate::api::AnnotationDraft| {
                existing.path == note.path
                    && existing.revision == note.revision
                    && existing.side == note.side
                    && existing.start_line == note.start_line
                    && existing.end_line == note.end_line
            })
        {
            return Err(invalid("annotation anchors must be unique"));
        }
        normalized.push(note.clone());
    }
    let digest = payload_hash_for_notes(&normalized)?;
    Ok((normalized, digest))
}

pub(super) fn payload_hash_for_notes(
    notes: &[crate::api::AnnotationDraft],
) -> Result<String, ApiError> {
    let canonical = notes
        .iter()
        .map(|note| NormalizedNote {
            path: &note.path,
            revision: &note.revision,
            side: note.side,
            start_line: note.start_line,
            end_line: note.end_line,
            excerpt: &note.excerpt,
            comment: &note.comment,
        })
        .collect::<Vec<_>>();
    let bytes =
        serde_json::to_vec(&canonical).map_err(|_| invalid("annotation payload is invalid"))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn validate_context(
    task_id: &TaskId,
    context: &TaskReviewContext,
    batch: &AnnotationBatch,
) -> Result<(), ApiError> {
    if &context.task_id != task_id {
        return Err(ApiError::new(
            ApiErrorCode::TaskNotFound,
            "review task context is unavailable",
            TRACE_ID,
        ));
    }
    if context.task_revision != batch.expected_task_revision {
        return Err(ApiError::new(
            ApiErrorCode::StaleCommand,
            "task revision is stale",
            TRACE_ID,
        ));
    }
    if context.active_run {
        return Err(ApiError::task_busy(TRACE_ID));
    }
    Ok(())
}

fn diff_lines(
    diff: &crate::api::DiffDocument,
    side: AnnotationSide,
    start: u32,
    end: u32,
) -> Vec<String> {
    diff.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| {
            match side {
                AnnotationSide::Old => line.old_line,
                AnnotationSide::New => line.new_line,
                AnnotationSide::File => None,
            }
            .is_some_and(|line| line >= start && line <= end)
        })
        .map(|line| line.text.clone())
        .collect()
}

fn invalid(message: &str) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequest, message, TRACE_ID)
}

fn outdated() -> ApiError {
    ApiError::new(
        ApiErrorCode::Outdated,
        "annotation anchor is outdated",
        TRACE_ID,
    )
}

fn payload_too_large() -> ApiError {
    ApiError::new(
        ApiErrorCode::PayloadTooLarge,
        "annotation batch is too large",
        TRACE_ID,
    )
}

fn idempotency_conflict() -> ApiError {
    ApiError::new(
        ApiErrorCode::IdempotencyConflict,
        "annotation idempotency key conflicts with an existing submission",
        TRACE_ID,
    )
}
