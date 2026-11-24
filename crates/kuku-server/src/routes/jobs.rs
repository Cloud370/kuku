use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kuku::{JobSnapshot, Session};
use serde::{Deserialize, Serialize};

use super::{Params, SessionTarget, manager};
use crate::AppState;
use crate::error_mapping::{ApiError, Result};

#[derive(Deserialize)]
pub(super) struct JobTarget {
    workspace: PathBuf,
    agent: String,
}

#[derive(Serialize)]
pub(super) struct Jobs {
    jobs: Vec<JobSnapshot>,
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<JobTarget>,
) -> Result<Json<Jobs>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let jobs = match manager(&state).await.open_session(&target.path) {
        Some(session) => session.jobs(&query.agent)?,
        None => Session::read_jobs(&target.path, &query.agent)?,
    };
    Ok(Json(Jobs { jobs }))
}

pub(super) async fn get(
    State(state): State<Arc<AppState>>,
    Path((id, job_id)): Path<(String, String)>,
    Params(query): Params<JobTarget>,
) -> Result<Json<JobSnapshot>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let job = match manager(&state).await.open_session(&target.path) {
        Some(session) => session.job(&query.agent, &job_id),
        None => Session::read_job(&target.path, &query.agent, &job_id),
    }
    .map_err(scoped_error)?;
    Ok(Json(job))
}

pub(super) async fn cancel(
    State(state): State<Arc<AppState>>,
    Path((id, job_id)): Path<(String, String)>,
    Params(query): Params<JobTarget>,
) -> Result<Json<JobSnapshot>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let job = match manager(&state).await.open_session(&target.path) {
        Some(session) => match session.cancel_job(&query.agent, &job_id).await {
            Ok(job) => return Ok(Json(job)),
            Err(kuku::Error::InvalidArgument(_)) => session.job(&query.agent, &job_id),
            Err(error) => return Err(error.into()),
        },
        None => Session::read_job(&target.path, &query.agent, &job_id),
    }
    .map_err(scoped_error)?;
    if job.finished_at.is_none() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "job_not_active",
            "This server has no live worker available to cancel the Job",
        ));
    }
    Ok(Json(job))
}

fn scoped_error(error: kuku::Error) -> ApiError {
    match error {
        kuku::Error::InvalidArgument(_) => {
            ApiError::new(StatusCode::NOT_FOUND, "job_not_found", "Job not found")
        }
        error => error.into(),
    }
}
