use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Query};
use axum::http::request::Parts;
use axum::routing::{delete, get, post, put};
use kuku::SessionOptions;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::AppState;
use crate::error_mapping::{ApiError, Result};
use crate::run_manager::RunManager;

mod commands;
mod events;
mod jobs;
mod models;
mod rewind_files;
mod rewind_history;
mod sessions;
mod values;
mod workspaces;

pub mod health;

pub(crate) fn interactive() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sessions", get(sessions::list))
        .route("/sessions/{id}", delete(sessions::delete))
        .route("/sessions/{id}/agents", get(sessions::agents))
        .route("/sessions/{id}/permissions", get(sessions::permissions))
        .route("/sessions/{id}/snapshot", get(sessions::snapshot))
        .route("/sessions/{id}/history", get(events::history))
        .route(
            "/sessions/{id}/history/destinations",
            get(events::history_destinations),
        )
        .route(
            "/sessions/{id}/history/destinations/{event_id}/files",
            get(rewind_files::preview),
        )
        .route(
            "/sessions/{id}/history/rewinds/{rewind_event_id}",
            get(rewind_history::history),
        )
        .route("/sessions/{id}/observe", get(events::observe))
        .route("/sessions/{id}/events", get(events::events))
        .route("/sessions/{id}/outputs/{output_id}", get(events::output))
        .route("/sessions/{id}/jobs", get(jobs::list))
        .route(
            "/sessions/{id}/jobs/{job_id}",
            get(jobs::get).delete(jobs::cancel),
        )
        .route("/sessions/{id}/values", get(values::list))
        .route(
            "/sessions/{id}/values/{name}",
            put(values::save).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/sessions/{id}/commands",
            post(commands::submit).layer(DefaultBodyLimit::max(
                2 * 1024 * 1024
                    + kuku::execution::MAX_INPUT_IMAGES
                        * kuku::execution::MAX_MODEL_IMAGE_BYTES.div_ceil(3)
                        * 4,
            )),
        )
        .route(
            "/sessions/{id}/receipts/{command_id}",
            get(commands::receipt),
        )
        .route("/models", get(models::list))
        .route("/models/resolve", get(models::resolve))
        .route("/workspaces", get(workspaces::list))
        .route("/workspaces/resolve", get(workspaces::resolve))
        .route(
            "/workspaces/permissions",
            get(workspaces::permission).put(workspaces::set_permission),
        )
        .route(
            "/workspaces/permissions/observe",
            get(events::observe_workspace_permissions),
        )
        .route("/workspaces/files", get(workspaces::files))
        .route("/workspaces/skills", get(workspaces::skills))
}

struct Params<T>(T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Params<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        let Query(params) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|error| ApiError::invalid(error.body_text()))?;
        Ok(Self(params))
    }
}

#[derive(Deserialize)]
struct WorkspaceQuery {
    workspace: PathBuf,
}

struct SessionTarget {
    workspace: PathBuf,
    path: PathBuf,
}

impl SessionTarget {
    fn new(state: &AppState, workspace: &Path, id: &str) -> Result<Self> {
        let workspace = workspaces::resolve_workspace(workspace)?;
        let home = canonical_home(&state.kuku_home)?;
        let path = kuku::session::session_events_path(&home, &workspace, id)?;
        Ok(Self { workspace, path })
    }

    fn require_exists(&self) -> Result<()> {
        if self.path.is_file() {
            Ok(())
        } else {
            Err(ApiError::session_not_found())
        }
    }
}

async fn manager(state: &AppState) -> RunManager {
    state.run_manager.lock().await.clone()
}

async fn journal_task<T: Send + 'static>(
    read: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(read)
        .await
        .map_err(std::io::Error::other)?
}

fn canonical_home(home: &Path) -> Result<PathBuf> {
    if home.is_dir() {
        Ok(home.canonicalize()?)
    } else {
        Ok(std::path::absolute(home)?)
    }
}

fn session_options(state: &AppState, workspace: PathBuf, id: String) -> SessionOptions {
    let mut options =
        SessionOptions::new(workspace, &state.kuku_home, (**state.config.load()).clone());
    options.session_id = Some(id);
    options.model_config_updates = Some(state.config.clone());
    options.authorizer = state.authorizer.clone();
    options
}
