use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ApiVersion, ContextSummary, Cursor, InteractionId, PageCursor, RequestId, ReviewSnapshot,
    ReviewSummaryProjection, RunId, RunState, TaskId, TaskRevision, TaskState, WorkspaceId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskSummary {
    pub task_id: TaskId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub state: TaskState,
    pub updated_at: String,
    #[serde(deserialize_with = "required_nullable")]
    pub active_run_id: Option<RunId>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_run_id: Option<RunId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileReferenceProjection {
    pub workspace_id: WorkspaceId,
    pub relative_path: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageProjection {
    pub message_id: String,
    pub role: MessageRole,
    pub text: String,
    pub finalized: bool,
    pub request_ids: Vec<RequestId>,
    pub file_references: Vec<FileReferenceProjection>,
    pub order_key: Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Tool,
    DelegatedAgent,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityProjection {
    pub activity_id: String,
    pub kind: ActivityKind,
    pub title: String,
    pub status: ActivityStatus,
    #[serde(deserialize_with = "required_nullable")]
    pub detail: Option<String>,
    pub file_references: Vec<FileReferenceProjection>,
    pub order_key: Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionChoiceProjection {
    pub choice_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    Pending,
    Resolved,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionProjection {
    pub interaction_id: InteractionId,
    pub prompt: String,
    pub choices: Vec<InteractionChoiceProjection>,
    #[serde(deserialize_with = "required_nullable")]
    pub selected_choice_id: Option<String>,
    pub status: InteractionStatus,
    pub order_key: Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "item", rename_all = "snake_case")]
pub enum TimelineItemProjection {
    Message(MessageProjection),
    Activity(ActivityProjection),
    Interaction(InteractionProjection),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckProjection {
    pub name: String,
    pub passed: bool,
    #[serde(deserialize_with = "required_nullable")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetricProjection {
    pub name: String,
    pub value: f64,
    #[serde(deserialize_with = "required_nullable")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompletionProjection {
    pub summary: String,
    #[serde(deserialize_with = "required_nullable")]
    pub checks: Option<Vec<CheckProjection>>,
    #[serde(deserialize_with = "required_nullable")]
    pub metrics: Option<Vec<MetricProjection>>,
    #[serde(deserialize_with = "required_nullable")]
    pub workspace_changes: Option<ReviewSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunProjection {
    pub run_id: RunId,
    pub state: RunState,
    pub started_at: String,
    #[serde(deserialize_with = "required_nullable")]
    pub finished_at: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub completion: Option<CompletionProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoadedSkillProjection {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub loaded_by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskProjection {
    pub api_version: ApiVersion,
    pub task_revision: TaskRevision,
    pub cursor: Cursor,
    pub task: TaskSummary,
    pub selected_tier_id: String,
    pub timeline: Vec<TimelineItemProjection>,
    #[serde(deserialize_with = "required_nullable")]
    pub timeline_next_cursor: Option<PageCursor>,
    pub loaded_skills: Vec<LoadedSkillProjection>,
    #[serde(deserialize_with = "required_nullable")]
    pub active_run: Option<RunProjection>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_run: Option<RunProjection>,
    #[serde(deserialize_with = "required_nullable")]
    pub context_summary: Option<ContextSummary>,
    pub review_summary: ReviewSummaryProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskPage {
    pub api_version: ApiVersion,
    pub items: Vec<TaskSummary>,
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListTasksQuery {
    pub workspace_id: WorkspaceId,
    #[serde(deserialize_with = "required_nullable")]
    pub search: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub cursor: Option<PageCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineQuery {
    #[serde(deserialize_with = "required_nullable")]
    pub before: Option<PageCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimelinePage {
    pub api_version: ApiVersion,
    pub task_id: TaskId,
    pub items: Vec<TimelineItemProjection>,
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineWindowDelta {
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<PageCursor>,
    pub evicted_items: Vec<TimelineItemProjection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskDelta {
    ProjectionReplaced {
        projection: Box<TaskProjection>,
    },
    ChangesApplied {
        changes: Vec<TaskChange>,
        #[serde(deserialize_with = "required_nullable")]
        timeline_window: Option<TimelineWindowDelta>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskChange {
    MessageAppended {
        item: TimelineItemProjection,
    },
    MessagePatched {
        message_id: String,
        append_text: String,
        finalized: bool,
        #[serde(deserialize_with = "required_nullable")]
        request_ids: Option<Vec<RequestId>>,
    },
    ActivityUpserted {
        activity: ActivityProjection,
    },
    InteractionUpserted {
        interaction: InteractionProjection,
    },
    RunStateChanged {
        task: TaskSummary,
        #[serde(deserialize_with = "required_nullable")]
        active_run: Option<Box<RunProjection>>,
        #[serde(deserialize_with = "required_nullable")]
        latest_run: Option<Box<RunProjection>>,
    },
    SkillsChanged {
        selected_tier_id: String,
        loaded_skills: Vec<LoadedSkillProjection>,
    },
    ContextSummaryChanged {
        #[serde(deserialize_with = "required_nullable")]
        context_summary: Option<ContextSummary>,
    },
    ReviewSubmissionsChanged {
        change: super::ReviewSubmissionsChanged,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskStreamEvent {
    pub api_version: ApiVersion,
    pub cursor: Cursor,
    pub task_revision: TaskRevision,
    pub task_id: TaskId,
    pub event: TaskDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskStreamQuery {
    #[serde(deserialize_with = "required_nullable")]
    pub after: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskRequest {
    pub workspace_id: WorkspaceId,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskResponse {
    pub api_version: ApiVersion,
    pub projection: TaskProjection,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmitRunRequest {
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
    pub message: String,
    pub tier_id: String,
    pub skill_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmitRunResponse {
    pub api_version: ApiVersion,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub task_revision: TaskRevision,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopRunRequest {
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InteractionResponseRequest {
    pub expected_task_revision: TaskRevision,
    pub idempotency_key: String,
    pub choice_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandAccepted {
    pub api_version: ApiVersion,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub replayed: bool,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}
