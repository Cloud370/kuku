use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::run_manager::HostReply;
use crate::AppState;

#[derive(Deserialize)]
pub struct ResponseRequest {
    #[serde(alias = "request_id")]
    pub interaction_id: String,
    pub choice: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, alias = "conversation")]
    pub agent: Option<String>,
    #[serde(default)]
    pub command_id: Option<String>,
    /// Session workspace required when replying through its persistent identity.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
}

pub async fn respond(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Json(body): Json<ResponseRequest>,
) -> Json<serde_json::Value> {
    let reply = match parse_reply(&body.choice, body.text.as_deref()) {
        Ok(reply) => reply,
        Err(error) => return Json(crate::error_mapping::error_envelope(&error)),
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

/// Reopen a durable Session and continue its exact pending question or authorization.
pub async fn respond_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(body): Json<ResponseRequest>,
) -> Response {
    let reply = match parse_reply(&body.choice, body.text.as_deref()) {
        Ok(reply) => reply,
        Err(error) => return Json(crate::error_mapping::error_envelope(&error)).into_response(),
    };
    let Some(workspace) = body.workspace else {
        return Json(json!({"ok":false,"code":"invalid_request","message":"workspace is required for a persistent Session reply"})).into_response();
    };
    if !workspace.is_dir() {
        return Json(json!({"ok":false,"code":"invalid_request","message":"workspace directory does not exist"})).into_response();
    }
    let events_path =
        match kuku::session::session_events_path(&state.kuku_home, &workspace, &session_id) {
            Ok(path) => path,
            Err(error) => {
                return Json(crate::error_mapping::error_envelope(&error)).into_response()
            }
        };
    if !events_path.exists() {
        return Json(json!({"ok":false,"code":"session_not_found","message":"Session not found"}))
            .into_response();
    }
    let mut options =
        kuku::SessionOptions::new(workspace, &state.kuku_home, (**state.config.load()).clone());
    options.session_id = Some(session_id);
    options.authorizer = state.authorizer.clone();
    let result = state
        .run_manager
        .lock()
        .await
        .spawn_reply(
            options,
            body.agent.unwrap_or_else(|| "main".into()),
            body.interaction_id,
            reply,
            body.command_id,
        )
        .await;
    match result {
        Ok(submission) => crate::routes::runs::submission_response(submission),
        Err(error) => Json(crate::error_mapping::error_envelope(&error)).into_response(),
    }
}

fn parse_reply(choice: &str, text: Option<&str>) -> kuku::Result<HostReply> {
    match choice {
        "once" => Ok(HostReply::Permission(true)),
        "deny" => Ok(HostReply::Permission(false)),
        "answer" => text
            .map(|text| HostReply::Question(text.into()))
            .ok_or_else(|| kuku::Error::InvalidArgument("answer requires text".into())),
        _ => Err(kuku::Error::InvalidArgument(
            "choice must be once, deny, or answer".into(),
        )),
    }
}
