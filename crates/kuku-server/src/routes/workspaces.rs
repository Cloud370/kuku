use std::collections::BTreeSet;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use kuku::permission::PermissionMode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Params;
use super::WorkspaceQuery;
use crate::AppState;
use crate::error_mapping::{ApiError, Result};

const COMPLETION_LIMIT: usize = 40;

pub(super) async fn permission(
    State(state): State<Arc<AppState>>,
    Params(query): Params<WorkspaceQuery>,
) -> Result<Json<Option<PermissionMode>>> {
    let workspace = resolve_workspace(&query.workspace)?;
    let manager = super::manager(&state).await;
    super::journal_task(move || {
        manager
            .workspace_permission(&state.kuku_home, &workspace)
            .map(Json)
    })
    .await
}

pub(super) async fn set_permission(
    State(state): State<Arc<AppState>>,
    Params(query): Params<WorkspaceQuery>,
    body: std::result::Result<Json<PermissionMode>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<PermissionMode>> {
    let Json(mode) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    let workspace = resolve_workspace(&query.workspace)?;
    let manager = super::manager(&state).await;
    super::journal_task(move || {
        manager.set_workspace_permission(&state.kuku_home, &workspace, mode)?;
        Ok(Json(mode))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct CompletionQuery {
    workspace: PathBuf,
    #[serde(default)]
    query: String,
}

pub(super) async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let mut workspaces = BTreeSet::new();
    if let Ok(workspace) = kuku::session::current_workspace() {
        if workspace.to_str().is_some() {
            workspaces.insert(workspace);
        }
    }
    for session in kuku::list_sessions(&state.kuku_home, None)? {
        if let Ok(workspace) = resolve_workspace(&session.workspace) {
            workspaces.insert(workspace);
        }
    }
    Ok(Json(json!({"ok":true,"workspaces":workspaces})))
}

pub(super) async fn resolve(Params(query): Params<WorkspaceQuery>) -> Result<Json<Value>> {
    let workspace = resolve_workspace(&query.workspace)?;
    Ok(Json(json!({"ok":true,"workspace":workspace})))
}

pub(super) async fn files(Params(query): Params<CompletionQuery>) -> Result<Json<Value>> {
    let workspace = resolve_workspace(&query.workspace)?;
    let paths =
        kuku::util::path::visible_workspace_paths(&workspace, &query.query, COMPLETION_LIMIT)?;
    Ok(Json(json!({"ok":true,"paths":paths})))
}

pub(super) async fn skills(
    State(state): State<Arc<AppState>>,
    Params(query): Params<CompletionQuery>,
) -> Result<Json<Value>> {
    let workspace = resolve_workspace(&query.workspace)?;
    let config = state.config.load();
    let registry =
        kuku::skill::build_registry_snapshot_for_host(&state.kuku_home, &workspace, &config)?;
    let query = query.query.to_lowercase();
    let skills = registry
        .definitions()
        .into_iter()
        .filter(|skill| {
            skill.name.to_lowercase().contains(&query)
                || skill.description.to_lowercase().contains(&query)
        })
        .take(COMPLETION_LIMIT)
        .map(|skill| json!({"name":skill.name,"description":skill.description}))
        .collect::<Vec<_>>();
    Ok(Json(json!({"ok":true,"skills":skills})))
}

pub(super) fn resolve_workspace(path: &FsPath) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(ApiError::invalid(
            "workspace must be an absolute server directory",
        ));
    }
    let resolved = path.canonicalize().map_err(|error| {
        ApiError::invalid(format!(
            "cannot resolve workspace '{}': {error}",
            path.display()
        ))
    })?;
    if !resolved.is_dir() || resolved.to_str().is_none() {
        return Err(ApiError::invalid(
            "workspace must be an existing UTF-8 server directory",
        ));
    }
    Ok(resolved)
}
