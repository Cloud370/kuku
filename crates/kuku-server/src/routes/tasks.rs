use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::{
    ApiError, Cursor, InteractionId, InteractionResponseRequest, ListTasksQuery, PageCursor,
    StopRunRequest, SubmitRunRequest, TaskId, TimelineQuery, WorkspaceId,
};
use crate::platform::BootstrapService;
use crate::run_manager::{
    DomainError, ResolveInteractionCommand, StopRunCommand, SubmitRunCommand, TaskRuntime,
};

#[derive(Clone)]
struct TaskHttpState {
    runtime: Arc<TaskRuntime>,
}

pub fn router(runtime: Arc<TaskRuntime>, bootstrap: Arc<BootstrapService>) -> Router {
    let state = Arc::new(TaskHttpState { runtime });
    Router::new()
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/tasks/{task_id}", get(task))
        .route("/tasks/{task_id}/timeline", get(timeline))
        .route("/tasks/{task_id}/runs", post(submit_run))
        .route("/tasks/{task_id}/stop", post(stop_run))
        .route(
            "/tasks/{task_id}/interactions/{interaction_id}",
            post(resolve_interaction),
        )
        .route("/tasks/{task_id}/stream", get(stream))
        .layer(middleware::from_fn_with_state(bootstrap, init_gate))
        .with_state(state)
}

async fn init_gate(
    State(bootstrap): State<Arc<BootstrapService>>,
    request: axum::http::Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    if !bootstrap.status().await.complete {
        let error = ApiError::new(
            crate::api::ApiErrorCode::InitIncomplete,
            "server initialization is incomplete",
            "task-init-gate",
        );
        return (StatusCode::CONFLICT, Json(error)).into_response();
    }
    next.run(request).await
}

#[derive(Deserialize)]
struct RawListTasksQuery {
    workspace_id: WorkspaceId,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    limit: u16,
}

#[derive(Deserialize)]
struct RawTimelineQuery {
    #[serde(default)]
    before: Option<String>,
    limit: u16,
}

#[derive(Deserialize)]
struct RawTaskStreamQuery {
    #[serde(default)]
    after: Option<String>,
}

async fn list_tasks(
    State(state): State<Arc<TaskHttpState>>,
    Query(query): Query<RawListTasksQuery>,
) -> Result<Json<crate::api::TaskPage>, TaskHttpError> {
    let cursor = query
        .cursor
        .map(PageCursor::try_new)
        .transpose()
        .map_err(|_| DomainError::InvalidRequest)?;
    Ok(Json(
        state
            .runtime
            .list_tasks_query(ListTasksQuery {
                workspace_id: query.workspace_id,
                search: query.search,
                cursor,
                limit: query.limit,
            })
            .await?,
    ))
}

async fn create_task(
    State(state): State<Arc<TaskHttpState>>,
    Json(request): Json<crate::api::CreateTaskRequest>,
) -> Result<(StatusCode, Json<crate::api::CreateTaskResponse>), TaskHttpError> {
    Ok((
        StatusCode::CREATED,
        Json(state.runtime.create_task(request).await?),
    ))
}

async fn task(
    State(state): State<Arc<TaskHttpState>>,
    Path(task_id): Path<TaskId>,
) -> Result<Json<crate::api::TaskProjection>, TaskHttpError> {
    Ok(Json(state.runtime.projection(&task_id).await?))
}

async fn timeline(
    State(state): State<Arc<TaskHttpState>>,
    Path(task_id): Path<TaskId>,
    Query(query): Query<RawTimelineQuery>,
) -> Result<Json<crate::api::TimelinePage>, TaskHttpError> {
    let before = query
        .before
        .map(PageCursor::try_new)
        .transpose()
        .map_err(|_| DomainError::InvalidRequest)?;
    Ok(Json(
        state
            .runtime
            .timeline(
                &task_id,
                TimelineQuery {
                    before,
                    limit: query.limit,
                },
            )
            .await?,
    ))
}

async fn submit_run(
    State(state): State<Arc<TaskHttpState>>,
    Path(task_id): Path<TaskId>,
    Json(request): Json<SubmitRunRequest>,
) -> Result<(StatusCode, Json<crate::api::SubmitRunResponse>), TaskHttpError> {
    if request.tier_id.is_empty() || request.tier_id.contains(':') {
        return Err(DomainError::InvalidRequest.into());
    }
    let result = state
        .runtime
        .submit(SubmitRunCommand {
            task_id,
            expected_task_revision: request.expected_task_revision,
            idempotency_key: request.idempotency_key,
            message: request.message,
            tier_id: format!("tier:{}", request.tier_id),
            skill_ids: request.skill_ids,
        })
        .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn stop_run(
    State(state): State<Arc<TaskHttpState>>,
    Path(task_id): Path<TaskId>,
    Json(request): Json<StopRunRequest>,
) -> Result<(StatusCode, Json<crate::api::CommandAccepted>), TaskHttpError> {
    let result = state
        .runtime
        .stop(StopRunCommand {
            task_id,
            expected_task_revision: request.expected_task_revision,
            idempotency_key: request.idempotency_key,
        })
        .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn resolve_interaction(
    State(state): State<Arc<TaskHttpState>>,
    Path((task_id, interaction_id)): Path<(TaskId, InteractionId)>,
    Json(request): Json<InteractionResponseRequest>,
) -> Result<(StatusCode, Json<crate::api::CommandAccepted>), TaskHttpError> {
    let result = state
        .runtime
        .resolve_interaction(ResolveInteractionCommand {
            task_id,
            interaction_id,
            choice_id: request.choice_id,
            expected_task_revision: request.expected_task_revision,
            idempotency_key: request.idempotency_key,
        })
        .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

async fn stream(
    State(state): State<Arc<TaskHttpState>>,
    Path(task_id): Path<TaskId>,
    Query(query): Query<RawTaskStreamQuery>,
) -> Result<Response, TaskHttpError> {
    let after = query
        .after
        .map(|raw| {
            raw.parse::<u64>()
                .map_err(|_| DomainError::InvalidRequest)
                .and_then(|value| Cursor::try_new(value).map_err(|_| DomainError::InvalidRequest))
        })
        .transpose()?;
    let mut subscription = state.runtime.subscribe(&task_id, after).await?;
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Vec<u8>, Infallible>>(8);
    tokio::spawn(async move {
        loop {
            let event = match subscription.next().await {
                Ok(event) => event,
                Err(_) => return,
            };
            let mut encoded = match serde_json::to_vec(&event) {
                Ok(encoded) => encoded,
                Err(_) => return,
            };
            encoded.push(b'\n');
            if sender.send(Ok(encoded)).await.is_err() {
                return;
            }
        }
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(ReceiverStream::new(receiver)))
        .expect("NDJSON response is valid"))
}

struct TaskHttpError(ApiError);

impl From<DomainError> for TaskHttpError {
    fn from(error: DomainError) -> Self {
        Self(error.into_api_error("task-http"))
    }
}

impl IntoResponse for TaskHttpError {
    fn into_response(self) -> Response {
        (status_for(self.0.code()), Json(self.0)).into_response()
    }
}

fn status_for(code: crate::api::ApiErrorCode) -> StatusCode {
    use crate::api::ApiErrorCode;
    match code {
        ApiErrorCode::TaskNotFound | ApiErrorCode::WorkspaceNotFound => StatusCode::NOT_FOUND,
        ApiErrorCode::TaskBusy
        | ApiErrorCode::StaleCommand
        | ApiErrorCode::IdempotencyConflict
        | ApiErrorCode::RunNotActive
        | ApiErrorCode::InteractionNotPending
        | ApiErrorCode::CursorAhead => StatusCode::CONFLICT,
        ApiErrorCode::ServerBusy | ApiErrorCode::StreamLimit => StatusCode::TOO_MANY_REQUESTS,
        ApiErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ApiErrorCode::StorageExhausted => StatusCode::INSUFFICIENT_STORAGE,
        ApiErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
