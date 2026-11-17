use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

#[derive(Deserialize)]
pub struct SessionsQuery {
    pub workspace: Option<String>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SessionsQuery>,
) -> Json<serde_json::Value> {
    let workspace = params.workspace.map(std::path::PathBuf::from);
    let sessions = match kuku::list_sessions(&state.kuku_home, workspace.as_deref()) {
        Ok(s) => s,
        Err(e) => return Json(json!({"ok": false, "code": "internal", "message": e.to_string()})),
    };

    let sessions_json: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "session_id": s.session_id,
                "workspace": s.workspace,
                "title": s.title,
                "created_at": s.created_at,
                "turn_count": s.turn_count,
                "status": s.status,
                "mtime": s.mtime,
                "size": s.size,
            })
        })
        .collect();

    Json(json!({"ok": true, "sessions": sessions_json}))
}

pub async fn conversations(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<SessionsQuery>,
) -> Json<serde_json::Value> {
    let workspace = match params.workspace {
        Some(ws) => std::path::PathBuf::from(ws),
        None => match kuku::session::current_workspace() {
            Ok(workspace) => workspace,
            Err(_) => {
                return Json(
                    json!({"ok": false, "code": "invalid_request", "message": "workspace parameter required"}),
                )
            }
        },
    };

    let events_path = match kuku::session::session_events_path(
        &state.kuku_home,
        &workspace,
        &session_id,
    ) {
        Ok(path) if path.exists() => path,
        _ => {
            return Json(
                json!({"ok": false, "code": "session_not_found", "message": "session not found"}),
            )
        }
    };

    let agents = match kuku::session::Session::read_agents(&events_path) {
        Ok(agents) => agents,
        Err(error) => return Json(crate::error_mapping::error_envelope(&error)),
    };
    let conversations = agents
        .iter()
        .map(|agent| {
            json!({
                "conversation": agent.address,
                "agent": agent.address,
                "status": agent.status,
                "pending_inputs": agent.pending_inputs,
                "permission": agent.permission,
                "question": agent.question,
                "config": agent.config,
                "usage": agent.usage,
            })
        })
        .collect::<Vec<_>>();

    Json(json!({"ok": true, "session_id": session_id, "conversations": conversations}))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<SessionsQuery>,
) -> Json<serde_json::Value> {
    let workspace = params.workspace.map(std::path::PathBuf::from);
    match kuku::delete_session(&state.kuku_home, workspace.as_deref(), &session_id) {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => {
            let code = match &e {
                kuku::Error::SessionLocked { .. } => "session_locked",
                kuku::Error::Io(io_err) if io_err.kind() == std::io::ErrorKind::NotFound => {
                    "session_not_found"
                }
                _ => "internal",
            };
            Json(json!({"ok": false, "code": code, "message": e.to_string()}))
        }
    }
}
