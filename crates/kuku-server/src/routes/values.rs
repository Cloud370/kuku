use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::Json;
use kuku::session::{NamedValue, NamedValueSource};
use kuku::{AgentConfig, Session};
use serde::{Deserialize, Serialize};

use super::commands::validate_agent_workspace;
use super::{manager, session_options, Params, SessionTarget};
use crate::error_mapping::{ApiError, Result};
use crate::run_manager::SaveValue as SaveValueIntent;
use crate::AppState;

#[derive(Deserialize)]
pub(super) struct ValueTarget {
    workspace: PathBuf,
    agent: String,
}

#[derive(Deserialize)]
pub(super) struct SaveValue {
    #[serde(flatten)]
    target: ValueTarget,
    source: NamedValueSource,
    #[serde(default)]
    replace: bool,
    create: Option<AgentConfig>,
}

#[derive(Serialize)]
pub(super) struct Values {
    values: Vec<NamedValue>,
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<ValueTarget>,
) -> Result<Json<Values>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let values = match manager(&state).await.open_session(&target.path) {
        Some(session) => session.named_values(&query.agent)?,
        None => Session::read_named_values(&target.path, &query.agent)?,
    };
    Ok(Json(Values { values }))
}

pub(super) async fn save(
    State(state): State<Arc<AppState>>,
    Path((id, name)): Path<(String, String)>,
    body: std::result::Result<Json<SaveValue>, JsonRejection>,
) -> Result<Json<NamedValue>> {
    let Json(mut request) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    let target = SessionTarget::new(&state, &request.target.workspace, &id)?;
    if let Some(config) = &mut request.create {
        if request.target.agent != "main" {
            return Err(ApiError::invalid(
                "Initialize the Session through its main Agent",
            ));
        }
        validate_agent_workspace(&target.workspace, config)?;
    } else {
        target.require_exists()?;
    }
    let options = session_options(&state, target.workspace, id);
    let result = manager(&state)
        .await
        .save_named_value(
            options,
            &target.path,
            &request.target.agent,
            &name,
            SaveValueIntent {
                source: request.source,
                replace: request.replace,
                create: request.create,
            },
        )
        .await?;
    Ok(Json(result))
}
