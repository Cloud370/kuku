use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AnnotationSide, ApiVersion, PageCursor, ReviewSubmissionId, RevisionToken, RunId, TaskId,
    TaskRevision, WorkspaceId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Untracked,
    Renamed,
    Copied,
    TypeChanged,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub kind: FileKind,
    #[schemars(required, range(max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "required_nullable_json_safe_u64")]
    pub size_bytes: Option<u64>,
    pub binary: bool,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub revision: Option<RevisionToken>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub change: Option<ChangeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilePage {
    pub api_version: ApiVersion,
    pub workspace_id: WorkspaceId,
    pub revision: RevisionToken,
    pub entries: Vec<FileEntry>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchMatch {
    pub entry: FileEntry,
    pub path_match_ranges: Vec<TextRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSearchPage {
    pub api_version: ApiVersion,
    pub workspace_id: WorkspaceId,
    pub revision: RevisionToken,
    pub matches: Vec<SearchMatch>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileContent {
    pub api_version: ApiVersion,
    pub workspace_id: WorkspaceId,
    pub path: String,
    pub revision: RevisionToken,
    pub start_line: u32,
    pub end_line: u32,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub total_lines: Option<u32>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub text: Option<String>,
    pub binary: bool,
    pub truncated: bool,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub next_start_line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangesAvailability {
    Available,
    NotGitRepository,
    GitUnavailable,
    WorkspaceRootMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeEntry {
    pub path: String,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub staged: bool,
    pub worktree: bool,
    pub binary: bool,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub additions: Option<u32>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub deletions: Option<u32>,
    pub revision: RevisionToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSnapshot {
    pub api_version: ApiVersion,
    pub workspace_id: WorkspaceId,
    pub revision: RevisionToken,
    pub availability: ChangesAvailability,
    pub entries: Vec<ChangeEntry>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    NoNewlineMarker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub old_line: Option<u32>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub new_line: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffDocument {
    pub api_version: ApiVersion,
    pub workspace_id: WorkspaceId,
    pub path: String,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub old_path: Option<String>,
    pub revision: RevisionToken,
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
    pub truncated: bool,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnnotationDraft {
    pub path: String,
    pub revision: RevisionToken,
    pub side: AnnotationSide,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnnotationBatch {
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
    pub notes: Vec<AnnotationDraft>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationStatus {
    Current,
    Outdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmittedReviewNote {
    pub path: String,
    pub revision: RevisionToken,
    pub side: AnnotationSide,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
    pub comment: String,
    pub status: AnnotationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSubmissionProjection {
    pub submission_id: ReviewSubmissionId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub task_revision: TaskRevision,
    pub submitted_at: String,
    pub notes: Vec<SubmittedReviewNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSubmissionResult {
    pub api_version: ApiVersion,
    pub submission: ReviewSubmissionProjection,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSubmissionPage {
    pub api_version: ApiVersion,
    pub task_id: TaskId,
    pub items: Vec<ReviewSubmissionProjection>,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSummaryProjection {
    pub total_submissions: u32,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub latest_submission_id: Option<ReviewSubmissionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSubmissionsChanged {
    pub submission: ReviewSubmissionProjection,
    pub total_submissions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileTreeQuery {
    pub prefix: String,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub cursor: Option<PageCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSearchQuery {
    pub query: String,
    pub prefix: String,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub cursor: Option<PageCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileContentQuery {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangesQuery {
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub cursor: Option<PageCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffQuery {
    pub path: String,
    pub revision: RevisionToken,
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub cursor: Option<PageCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewSubmissionQuery {
    #[schemars(required)]
    #[serde(deserialize_with = "required_nullable")]
    pub cursor: Option<PageCursor>,
    pub limit: u16,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

fn required_nullable_json_safe_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<u64>::deserialize(deserializer)? {
        Some(value) if value > 9_007_199_254_740_991 => Err(serde::de::Error::custom(
            "integer exceeds JavaScript safe maximum",
        )),
        value => Ok(value),
    }
}
