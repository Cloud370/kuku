use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

#[derive(Deserialize)]
pub struct EventsQuery {
    pub after: Option<u64>,
    pub workspace: Option<String>,
}

pub async fn events(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<EventsQuery>,
) -> Json<serde_json::Value> {
    let home = &state.kuku_home;

    let workspace = match params.workspace {
        Some(ws) => std::path::PathBuf::from(ws),
        None => match kuku::session::current_workspace() {
            Ok(w) => w,
            Err(_) => {
                return Json(
                    json!({"ok": false, "code": "invalid_request", "message": "workspace parameter required"}),
                )
            }
        },
    };

    let events_path = match kuku::session::session_events_path(home, &workspace, &session_id) {
        Ok(p) => p,
        Err(_) => {
            return Json(
                json!({"ok": false, "code": "session_not_found", "message": "session not found"}),
            )
        }
    };

    if !events_path.exists() {
        return Json(
            json!({"ok": false, "code": "session_not_found", "message": "session not found"}),
        );
    }

    let events = match kuku::event::EventStore::replay(&events_path) {
        Ok(e) => e,
        Err(e) => return Json(json!({"ok": false, "code": "internal", "message": e.to_string()})),
    };

    let after = params.after.unwrap_or(0);
    let filtered: Vec<_> = events
        .iter()
        .filter(|e| e.id > after)
        .map(|e| {
            json!({
                "id": e.id,
                "payload": serde_json::to_value(&e.payload).unwrap_or_default(),
            })
        })
        .collect();

    let active_stream: Vec<serde_json::Value> = {
        let mgr = state.run_manager.lock().await;
        mgr.recent_events(&session_id)
            .into_iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
            .collect()
    };

    if active_stream.is_empty() {
        Json(json!(filtered))
    } else {
        Json(json!({
            "events": filtered,
            "active_stream": active_stream,
        }))
    }
}
