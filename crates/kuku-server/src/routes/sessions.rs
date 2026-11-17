use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use kuku::adapter::inspect_model;
use kuku::event::EventStore;
use kuku::session::{AgentSnapshot, AgentStatus, Session, SessionStatus};
use serde::Deserialize;
use serde_json::{json, Value};

use super::workspaces::resolve_workspace;
use super::Params;
use super::{canonical_home, manager, SessionTarget, WorkspaceQuery};
use crate::error_mapping::{ApiError, Result};
use crate::wire::agent_summary;
use crate::AppState;

const TITLE_CHARS: usize = 160;

#[derive(Deserialize)]
pub(super) struct ListQuery {
    workspace: Option<PathBuf>,
    search: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct SnapshotQuery {
    workspace: PathBuf,
    agent: Option<String>,
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    Params(query): Params<ListQuery>,
) -> Result<Json<Value>> {
    let workspace = query
        .workspace
        .as_deref()
        .map(resolve_workspace)
        .transpose()?;
    let home = canonical_home(&state.kuku_home)?;
    let search = query.search.unwrap_or_default().to_lowercase();
    let manager = manager(&state).await;
    let mut sessions = Vec::new();
    for summary in kuku::list_sessions(&home, workspace.as_deref())? {
        let path =
            kuku::session::session_events_path(&home, &summary.workspace, &summary.session_id)?;
        let agents = if let Some(session) = manager.open_session(&path) {
            session.agents()
        } else {
            Session::read_agents(&path)?
        };
        let title = title(&summary.title, &path, &agents)?;
        let status = session_status(&agents, summary.status);
        if !search.is_empty()
            && !title.to_lowercase().contains(&search)
            && !summary.session_id.to_lowercase().contains(&search)
        {
            continue;
        }
        let waiting_agents = agents
            .iter()
            .filter(|agent| agent.status == AgentStatus::Waiting)
            .map(|agent| &agent.address)
            .collect::<Vec<_>>();
        sessions.push(json!({
            "session_id":summary.session_id,
            "workspace":summary.workspace,
            "title":title,
            "created_at":summary.created_at,
            "turn_count":summary.turn_count,
            "status":status,
            "mtime":summary.mtime,
            "size":summary.size,
            "waiting_agents":waiting_agents,
        }));
    }
    Ok(Json(json!({"ok":true,"sessions":sessions})))
}

pub(super) async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<WorkspaceQuery>,
) -> Result<Json<Value>> {
    let workspace = resolve_workspace(&query.workspace)?;
    let home = canonical_home(&state.kuku_home)?;
    manager(&state)
        .await
        .delete_session(&home, &workspace, &id)
        .await?;
    Ok(Json(json!({"ok":true})))
}

pub(super) async fn agents(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<WorkspaceQuery>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let initial_cursor = EventStore::read_head(&target.path)?;
    let open = manager(&state).await.open_session(&target.path);
    let agents = match &open {
        Some(session) => session.agents(),
        None => Session::read_agents(&target.path)?,
    };
    let (agents, cursor) = if let Some(first) = agents.first() {
        let view = match &open {
            Some(session) => session.agent_view(&first.address)?,
            None => Session::read_view(&target.path, &first.address)?,
        };
        (view.agents, view.cursor)
    } else {
        (agents, initial_cursor)
    };
    let agents: Vec<_> = agents.iter().map(agent_summary).collect();
    Ok(Json(
        json!({"ok":true,"session_id":id,"workspace":target.workspace,"cursor":cursor,"agents":agents}),
    ))
}

pub(super) async fn snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<SnapshotQuery>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let address = query.agent.as_deref().unwrap_or("main");
    let manager = manager(&state).await;
    let mut open = manager.open_session(&target.path);
    let view = match &open {
        Some(session) => session.agent_view(address),
        None => {
            let read = Session::read_view(&target.path, address);
            open = manager.open_session(&target.path);
            open.as_ref()
                .map_or(read, |session| session.agent_view(address))
        }
    }
    .map_err(agent_error)?;
    let agents: Vec<_> = view.agents.iter().map(agent_summary).collect();
    let mut value = json!({
        "ok":true,
        "session_id":id,
        "workspace":target.workspace,
        "cursor":view.cursor,
        "agent":view.snapshot,
        "agents":agents,
        "pending":view.pending_inputs,
        "selection":null,
    });
    let effective = view
        .snapshot
        .active_config
        .as_ref()
        .unwrap_or(&view.snapshot.config);
    match selection(&state, open.as_ref(), address, effective) {
        Ok(selection) => value["selection"] = selection,
        Err(error) => value["selection_error"] = ApiError::from(error).value(),
    }
    if effective != &view.snapshot.config {
        match selection(&state, open.as_ref(), address, &view.snapshot.config) {
            Ok(selection) => value["pending_selection"] = selection,
            Err(error) => value["pending_selection_error"] = ApiError::from(error).value(),
        }
    }
    if let Some(session) = &open {
        value["cleaning_up"] = json!(
            view.snapshot.status == AgentStatus::Stopped
                && session
                    .active_executions()
                    .iter()
                    .any(|(address, _)| address == &view.snapshot.address)
        );
    }
    Ok(Json(value))
}

fn selection(
    state: &AppState,
    open: Option<&Session>,
    address: &str,
    config: &kuku::AgentConfig,
) -> kuku::Result<Value> {
    if let Some(session) = open {
        return Ok(serde_json::to_value(
            session.selection_for_config(address, config.clone())?,
        )?);
    }
    let mut selection = serde_json::to_value(inspect_model(&state.config.load(), config)?)?;
    selection["tools"] = json!([]);
    Ok(selection)
}

fn agent_error(error: kuku::Error) -> ApiError {
    if matches!(error, kuku::Error::InvalidArgument(_)) {
        ApiError::new(StatusCode::NOT_FOUND, "agent_not_found", error.to_string())
    } else {
        error.into()
    }
}

fn session_status(agents: &[AgentSnapshot], fallback: SessionStatus) -> SessionStatus {
    if agents
        .iter()
        .any(|agent| agent.status == AgentStatus::Running)
    {
        SessionStatus::Active
    } else if agents.iter().any(|agent| {
        matches!(
            agent.status,
            AgentStatus::Interrupted | AgentStatus::Stopped
        )
    }) {
        SessionStatus::Interrupted
    } else if agents.iter().any(|agent| agent.turn.is_some()) {
        SessionStatus::Done
    } else {
        fallback
    }
}

fn title(admitted: &str, path: &FsPath, agents: &[AgentSnapshot]) -> Result<String> {
    if !admitted.is_empty() {
        return Ok(admitted.chars().take(TITLE_CHARS).collect());
    }
    if let Some(agent) = agents.iter().find(|agent| agent.pending_inputs > 0) {
        let view = Session::read_view(path, &agent.address)?;
        if let Some(first) = view.pending_inputs.first() {
            return Ok(first.input.text.chars().take(TITLE_CHARS).collect());
        }
    }
    Ok(String::new())
}
