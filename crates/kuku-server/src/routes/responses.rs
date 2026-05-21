use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

#[derive(Deserialize)]
pub struct ResponseRequest {
    pub interaction_id: String,
    pub choice: String,
}

pub async fn respond(
    State(state): State<Arc<AppState>>,
    Path(_run_id): Path<String>,
    Json(body): Json<ResponseRequest>,
) -> Json<serde_json::Value> {
    let choice = match body.choice.as_str() {
        "once" => kuku::PermissionChoice::Once,
        "session" => kuku::PermissionChoice::Session,
        "project" => kuku::PermissionChoice::Project,
        "deny" => kuku::PermissionChoice::Deny,
        _ => {
            return Json(
                json!({"ok": false, "code": "invalid_request", "message": "invalid choice"}),
            )
        }
    };

    let mgr = state.run_manager.lock().await;
    match mgr.respond(&body.interaction_id, choice).await {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => Json(crate::error_mapping::error_envelope(&e)),
    }
}
