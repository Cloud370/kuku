use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ReviewSubmissionId, RevisionToken, RunId, TaskId, TaskRevision};

/// Identifies the content side anchored by a review annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationSide {
    File,
    Old,
    New,
}

/// Preserves one submitted review annotation as an immutable fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewAnnotationFact {
    pub path: String,
    pub revision: RevisionToken,
    pub side: AnnotationSide,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
    pub comment: String,
}

/// Records one ordered batch of review annotations committed to a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSubmissionRecorded {
    pub submission_id: ReviewSubmissionId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub task_revision: TaskRevision,
    pub submitted_at: String,
    pub notes: Vec<ReviewAnnotationFact>,
}
