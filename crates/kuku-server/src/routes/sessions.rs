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
    State(_state): State<Arc<AppState>>,
    Query(params): Query<SessionsQuery>,
) -> Json<serde_json::Value> {
    let home = match kuku::session::kuku_home() {
        Ok(h) => h,
        Err(_) => {
            return Json(json!({"ok": false, "code": "internal", "message": "missing home"}))
        }
    };

    let workspace = params.workspace.map(std::path::PathBuf::from);
    let sessions = match kuku::list_sessions(&home, workspace.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            return Json(json!({"ok": false, "code": "internal", "message": e.to_string()}))
        }
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

pub async fn delete(
    State(_state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<SessionsQuery>,
) -> Json<serde_json::Value> {
    let home = match kuku::session::kuku_home() {
        Ok(h) => h,
        Err(_) => {
            return Json(json!({"ok": false, "code": "internal", "message": "missing home"}))
        }
    };

    let workspace = params.workspace.map(std::path::PathBuf::from);
    match kuku::delete_session(&home, workspace.as_deref(), &session_id) {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => {
            let code = match &e {
                kuku::Error::SessionLocked { .. } => "session_locked",
                kuku::Error::Io(io_err)
                    if io_err.kind() == std::io::ErrorKind::NotFound =>
                {
                    "session_not_found"
                }
                _ => "internal",
            };
            Json(json!({"ok": false, "code": code, "message": e.to_string()}))
        }
    }
}
