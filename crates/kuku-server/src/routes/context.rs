use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::api::{ApiError, ApiErrorCode, RequestId, TaskId};
use crate::context::read_model::{ContextReadError, ContextReadModel};

pub(crate) fn router<S>(model: Arc<ContextReadModel>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/tasks/{task_id}/context", get(current))
        .route("/tasks/{task_id}/context/{request_id}", get(historical))
        .with_state(model)
        .layer(axum::middleware::map_response(no_store_response))
}

pub(crate) async fn current(
    State(model): State<Arc<ContextReadModel>>,
    Path(task_id): Path<String>,
) -> Result<Response, ContextHttpError> {
    let task_id = TaskId::parse(task_id).map_err(|_| ContextReadError::InvalidRequest)?;
    let snapshot = model.snapshot(&task_id, None)?;
    Ok(json_no_store(snapshot))
}

pub(crate) async fn historical(
    State(model): State<Arc<ContextReadModel>>,
    Path((task_id, request_id)): Path<(String, String)>,
) -> Result<Response, ContextHttpError> {
    let task_id = TaskId::parse(task_id).map_err(|_| ContextReadError::InvalidRequest)?;
    let request_id = RequestId::parse(request_id).map_err(|_| ContextReadError::InvalidRequest)?;
    let snapshot = model.snapshot(&task_id, Some(&request_id))?;
    Ok(json_no_store(snapshot))
}

#[derive(Debug)]
pub(crate) struct ContextHttpError(pub(crate) ContextReadError);

impl From<ContextReadError> for ContextHttpError {
    fn from(error: ContextReadError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ContextHttpError {
    fn into_response(self) -> Response {
        let (code, status) = match self.0 {
            ContextReadError::InvalidRequest => {
                (ApiErrorCode::InvalidRequest, StatusCode::BAD_REQUEST)
            }
            ContextReadError::TaskNotFound => (ApiErrorCode::TaskNotFound, StatusCode::NOT_FOUND),
            ContextReadError::RequestNotFound => {
                (ApiErrorCode::RequestNotFound, StatusCode::NOT_FOUND)
            }
            ContextReadError::ConversationNotFound => {
                (ApiErrorCode::ConversationNotFound, StatusCode::NOT_FOUND)
            }
            ContextReadError::WorkspaceNotFound => {
                (ApiErrorCode::WorkspaceNotFound, StatusCode::NOT_FOUND)
            }
            ContextReadError::LedgerCorrupt | ContextReadError::UsageCorrupt => {
                (ApiErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR)
            }
        };
        json_no_store_with_status(
            ApiError::new(code, self.0.to_string(), "context-read"),
            status,
        )
    }
}

pub(crate) async fn no_store_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

pub(crate) fn json_no_store<T: serde::Serialize>(value: T) -> Response {
    json_no_store_with_status(value, StatusCode::OK)
}

pub(crate) fn json_no_store_with_status<T: serde::Serialize>(
    value: T,
    status: StatusCode,
) -> Response {
    let mut response = (status, Json(value)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
