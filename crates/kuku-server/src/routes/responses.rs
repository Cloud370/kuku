use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::run_manager::HostReply;
use crate::AppState;

#[derive(Deserialize)]
pub struct ResponseRequest {
    pub interaction_id: String,
    pub choice: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, alias = "conversation")]
    pub agent: Option<String>,
    #[serde(default)]
    pub command_id: Option<String>,
}

pub async fn respond(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(body): Json<ResponseRequest>,
) -> Json<serde_json::Value> {
    let reply = match body.choice.as_str() {
        "once" => HostReply::Permission(true),
        "deny" => HostReply::Permission(false),
        "answer" => match body.text {
            Some(text) => HostReply::Question(text),
            None => {
                return Json(
                    json!({"ok":false,"code":"invalid_request","message":"answer requires text"}),
                )
            }
        },
        _ => {
            return Json(
                json!({"ok":false,"code":"invalid_request","message":"choice must be once, deny, or answer"}),
            )
        }
    };
    let command_id = body
        .command_id
        .unwrap_or_else(kuku::session::new_session_id);
    let manager = state.run_manager.lock().await;
    match manager.respond(
        &run_id,
        &body.interaction_id,
        body.agent.as_deref(),
        &command_id,
        reply,
    ) {
        Ok(receipt) => Json(json!({"ok":true,"receipt":receipt})),
        Err(error) => Json(crate::error_mapping::error_envelope(&error)),
    }
}
