use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use kuku::session::{FileRewindPreview, Session};
use serde::Deserialize;

use super::{Params, SessionTarget};
use crate::AppState;
use crate::error_mapping::{ApiError, Result};

#[derive(Deserialize)]
pub(super) struct Query {
    workspace: PathBuf,
    agent: String,
}

pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Path((id, event_id)): Path<(String, u64)>,
    Params(query): Params<Query>,
) -> Result<Json<FileRewindPreview>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let preview = tokio::task::spawn_blocking(move || {
        Session::preview_file_rewind(&target.path, &target.workspace, &query.agent, event_id)
    })
    .await
    .map_err(|error| ApiError::from(kuku::Error::InvalidEventStream(error.to_string())))??;
    Ok(Json(preview))
}
