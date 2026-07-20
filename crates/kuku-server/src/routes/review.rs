//! Exposes the seven typed Review HTTP handlers.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::api::{
    AnnotationBatch, ApiError, ApiErrorCode, DiffDocument, FileContent, FilePage, FileSearchPage,
    PageCursor, ReviewSnapshot, ReviewSubmissionPage, ReviewSubmissionResult, RevisionToken,
    TaskId, WorkspaceId,
};
use crate::review::annotations::AnnotationService;
use crate::review::files::WorkspaceReadService;
use crate::review::git::GitReviewService;
use crate::review::submissions::ReviewSubmissionService;
use crate::review::{ReviewAdmission, ReviewLimits, WorkspaceCapabilityProvider};

/// Holds Review services shared by all typed handlers.
pub struct ReviewRouteState {
    workspaces: Arc<dyn WorkspaceCapabilityProvider>,
    files: Arc<WorkspaceReadService>,
    annotations: Arc<AnnotationService>,
    submissions: Arc<ReviewSubmissionService>,
    admission: Arc<ReviewAdmission>,
    limits: ReviewLimits,
}

impl ReviewRouteState {
    /// Creates the shared state for the Review route group.
    pub fn new(
        workspaces: Arc<dyn WorkspaceCapabilityProvider>,
        files: Arc<WorkspaceReadService>,
        annotations: Arc<AnnotationService>,
        submissions: Arc<ReviewSubmissionService>,
        limits: ReviewLimits,
    ) -> Self {
        let admission = Arc::new(ReviewAdmission::new(&limits));
        Self::with_admission(
            workspaces,
            files,
            annotations,
            submissions,
            admission,
            limits,
        )
    }

    /// Creates route state using admission pools shared with its services.
    pub fn with_admission(
        workspaces: Arc<dyn WorkspaceCapabilityProvider>,
        files: Arc<WorkspaceReadService>,
        annotations: Arc<AnnotationService>,
        submissions: Arc<ReviewSubmissionService>,
        admission: Arc<ReviewAdmission>,
        limits: ReviewLimits,
    ) -> Self {
        Self {
            workspaces,
            files,
            annotations,
            submissions,
            admission,
            limits,
        }
    }
}

/// Builds the seven endpoint Review route group for integration mounting.
pub fn router<S>(state: Arc<ReviewRouteState>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/workspaces/{workspace_id}/files/tree", get(tree))
        .route("/workspaces/{workspace_id}/files/search", get(search))
        .route("/workspaces/{workspace_id}/files/content", get(content))
        .route("/workspaces/{workspace_id}/changes", get(changes))
        .route("/workspaces/{workspace_id}/changes/diff", get(diff))
        .route("/tasks/{task_id}/review/submissions", get(submissions))
        .route("/tasks/{task_id}/review/annotations", post(annotations))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct TreeQuery {
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    cursor: Option<String>,
    limit: u16,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(rename = "q")]
    query: String,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    cursor: Option<String>,
    limit: u16,
}

#[derive(Debug, Deserialize)]
struct ContentQuery {
    path: String,
    start_line: u32,
    end_line: u32,
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    #[serde(default)]
    cursor: Option<String>,
    limit: u16,
}

#[derive(Debug, Deserialize)]
struct DiffQuery {
    path: String,
    revision: RevisionToken,
    #[serde(default)]
    cursor: Option<String>,
    limit: u16,
}

#[derive(Debug, Deserialize)]
struct SubmissionsQuery {
    #[serde(default)]
    cursor: Option<String>,
    limit: u16,
}

async fn tree(
    State(state): State<Arc<ReviewRouteState>>,
    Path(workspace_id): Path<WorkspaceId>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<FilePage>, ReviewHttpError> {
    let cursor = parse_cursor(query.cursor)?;
    Ok(Json(
        state
            .files
            .tree(&workspace_id, &query.prefix, cursor.as_ref(), query.limit)
            .await?,
    ))
}

async fn search(
    State(state): State<Arc<ReviewRouteState>>,
    Path(workspace_id): Path<WorkspaceId>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<FileSearchPage>, ReviewHttpError> {
    let cursor = parse_cursor(query.cursor)?;
    Ok(Json(
        state
            .files
            .search(
                &workspace_id,
                &query.prefix,
                &query.query,
                cursor.as_ref(),
                query.limit,
            )
            .await?,
    ))
}

async fn content(
    State(state): State<Arc<ReviewRouteState>>,
    Path(workspace_id): Path<WorkspaceId>,
    Query(query): Query<ContentQuery>,
) -> Result<Json<FileContent>, ReviewHttpError> {
    Ok(Json(
        state
            .files
            .content(&workspace_id, &query.path, query.start_line, query.end_line)
            .await?,
    ))
}

async fn changes(
    State(state): State<Arc<ReviewRouteState>>,
    Path(workspace_id): Path<WorkspaceId>,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<ReviewSnapshot>, ReviewHttpError> {
    let cursor = parse_cursor(query.cursor)?;
    let capability = state.workspaces.capability(&workspace_id)?;
    Ok(Json(
        GitReviewService::with_admission(capability, state.limits.clone(), state.admission.clone())
            .snapshot(cursor.as_ref(), query.limit)
            .await?,
    ))
}

async fn diff(
    State(state): State<Arc<ReviewRouteState>>,
    Path(workspace_id): Path<WorkspaceId>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<DiffDocument>, ReviewHttpError> {
    let cursor = parse_cursor(query.cursor)?;
    let capability = state.workspaces.capability(&workspace_id)?;
    Ok(Json(
        GitReviewService::with_admission(capability, state.limits.clone(), state.admission.clone())
            .diff(&query.path, &query.revision, cursor.as_ref(), query.limit)
            .await?,
    ))
}

async fn submissions(
    State(state): State<Arc<ReviewRouteState>>,
    Path(task_id): Path<TaskId>,
    Query(query): Query<SubmissionsQuery>,
) -> Result<Json<ReviewSubmissionPage>, ReviewHttpError> {
    let cursor = parse_cursor(query.cursor)?;
    Ok(Json(
        state
            .submissions
            .list(&task_id, cursor.as_ref(), query.limit)
            .await?,
    ))
}

async fn annotations(
    State(state): State<Arc<ReviewRouteState>>,
    Path(task_id): Path<TaskId>,
    Json(batch): Json<AnnotationBatch>,
) -> Result<(StatusCode, Json<ReviewSubmissionResult>), ReviewHttpError> {
    Ok((
        StatusCode::CREATED,
        Json(state.annotations.submit(&task_id, batch).await?),
    ))
}

fn parse_cursor(raw: Option<String>) -> Result<Option<PageCursor>, ReviewHttpError> {
    raw.map(PageCursor::try_new)
        .transpose()
        .map_err(|_| invalid_request().into())
}

struct ReviewHttpError(ApiError);

impl From<ApiError> for ReviewHttpError {
    fn from(error: ApiError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ReviewHttpError {
    fn into_response(self) -> Response {
        (status_for(self.0.code()), Json(self.0)).into_response()
    }
}

fn status_for(code: ApiErrorCode) -> StatusCode {
    match code {
        ApiErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        ApiErrorCode::WorkspaceNotFound
        | ApiErrorCode::TaskNotFound
        | ApiErrorCode::FileNotFound => StatusCode::NOT_FOUND,
        ApiErrorCode::Outdated
        | ApiErrorCode::StaleCommand
        | ApiErrorCode::TaskBusy
        | ApiErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        ApiErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ApiErrorCode::ServerBusy => StatusCode::SERVICE_UNAVAILABLE,
        ApiErrorCode::WorkspaceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ApiErrorCode::AuthRequired
        | ApiErrorCode::Forbidden
        | ApiErrorCode::OriginNotAllowed
        | ApiErrorCode::InitIncomplete
        | ApiErrorCode::StaleServerRevision
        | ApiErrorCode::WorkspaceInUse
        | ApiErrorCode::RequestNotFound
        | ApiErrorCode::ConversationNotFound
        | ApiErrorCode::RunNotActive
        | ApiErrorCode::InteractionNotPending
        | ApiErrorCode::CursorAhead
        | ApiErrorCode::UnsupportedMediaType
        | ApiErrorCode::StreamLimit
        | ApiErrorCode::ProviderUnavailable
        | ApiErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::StorageExhausted => StatusCode::INSUFFICIENT_STORAGE,
    }
}

fn invalid_request() -> ApiError {
    ApiError::new(
        ApiErrorCode::InvalidRequest,
        "review cursor is invalid",
        "review-http",
    )
}
