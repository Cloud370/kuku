use std::sync::Arc;

use kuku::event::{
    ReviewAnnotationFact, ReviewSubmissionId, ReviewSubmissionRecorded, RunId, SkillContextFact,
    SkillsChangedFact, TaskId, TaskRevision, WorkspaceId,
};

use crate::api::{
    AnnotationStatus, ApiVersion, ReviewSubmissionProjection, ReviewSubmissionResult,
    SubmittedReviewNote,
};

use super::DomainError;

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
pub struct SubmitReviewCommand {
    pub task_id: TaskId,
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
    pub submission_id: ReviewSubmissionId,
    pub payload_hash: String,
    pub message: String,
    pub notes: Vec<ReviewAnnotationFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSkillSelection {
    pub selection: SkillsChangedFact,
    pub selected_skills: Vec<SkillContextFact>,
}

pub trait SkillSelectionValidator: Send + Sync {
    fn validate(
        &self,
        workspace_id: &WorkspaceId,
        tier_id: &str,
        skill_ids: &[String],
    ) -> Result<ValidatedSkillSelection, DomainError>;
}

pub trait ReviewSubmissionValidator: Send + Sync {
    fn validate(
        &self,
        task_id: &TaskId,
        submission_id: &ReviewSubmissionId,
        notes: &[ReviewAnnotationFact],
    ) -> Result<Vec<ReviewAnnotationFact>, DomainError>;
}

pub trait RunQueueAdmission: Send + Sync {
    fn reserve(self: Arc<Self>) -> Result<Box<dyn RunQueueReservation>, DomainError>;
    fn ensure_admitted(&self, task_id: &TaskId, run_id: &RunId) -> Result<(), DomainError>;
}

pub trait RunQueueReservation: Send {
    fn commit(self: Box<Self>, task_id: TaskId, run_id: RunId);
}

pub(super) fn review_result(
    recorded: ReviewSubmissionRecorded,
    replayed: bool,
) -> ReviewSubmissionResult {
    ReviewSubmissionResult {
        api_version: ApiVersion,
        submission: ReviewSubmissionProjection {
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
        },
        replayed,
    }
}

#[cfg(test)]
pub(super) struct TestSkillValidator;

#[cfg(test)]
impl SkillSelectionValidator for TestSkillValidator {
    fn validate(
        &self,
        _: &WorkspaceId,
        tier_id: &str,
        skill_ids: &[String],
    ) -> Result<ValidatedSkillSelection, DomainError> {
        Ok(ValidatedSkillSelection {
            selection: SkillsChangedFact {
                tier_id: tier_id.to_owned(),
                skill_ids: skill_ids.to_vec(),
            },
            selected_skills: Vec::new(),
        })
    }
}

#[cfg(test)]
pub(super) struct TestReviewValidator;

#[cfg(test)]
impl ReviewSubmissionValidator for TestReviewValidator {
    fn validate(
        &self,
        _: &TaskId,
        _: &ReviewSubmissionId,
        notes: &[ReviewAnnotationFact],
    ) -> Result<Vec<ReviewAnnotationFact>, DomainError> {
        Ok(notes.to_vec())
    }
}

#[cfg(test)]
pub(super) struct TestQueue;

#[cfg(test)]
impl RunQueueAdmission for TestQueue {
    fn reserve(self: Arc<Self>) -> Result<Box<dyn RunQueueReservation>, DomainError> {
        Ok(Box::new(TestReservation))
    }

    fn ensure_admitted(&self, _: &TaskId, _: &RunId) -> Result<(), DomainError> {
        Ok(())
    }
}

#[cfg(test)]
struct TestReservation;

#[cfg(test)]
impl RunQueueReservation for TestReservation {
    fn commit(self: Box<Self>, _: TaskId, _: RunId) {}
}
