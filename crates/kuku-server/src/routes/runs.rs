use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use kuku::session::{AgentConfig, SessionOptions};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

use crate::run_manager::RunSubmission;
use crate::AppState;

#[derive(Deserialize)]
pub struct RunRequest {
    pub prompt: String,
    pub workspace: PathBuf,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default, alias = "conversation")]
    pub agent: Option<String>,
    #[serde(default)]
    pub command_id: Option<String>,
    #[serde(default)]
    pub connection: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub no_agents: bool,
    #[serde(default)]
    pub no_skills: bool,
}

pub async fn create_run(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RunRequest>,
) -> Response {
    if !body.workspace.is_dir() {
        return Json(json!({"ok": false, "code": "invalid_request", "message": "workspace directory does not exist"})).into_response();
    }
    if body
        .command_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 100)
    {
        return Json(json!({"ok":false,"code":"invalid_request","message":"command_id must contain 1..=100 bytes"})).into_response();
    }
    let config = (**state.config.load()).clone();
    let mut options = SessionOptions::new(body.workspace, &state.kuku_home, config);
    options.session_id = body.session_id;
    options.authorizer = state.authorizer.clone();
    let agent = body.agent.unwrap_or_else(|| "main".into());
    let agent_config = AgentConfig {
        connection: body.connection,
        model: body.model,
        tier: body.tier,
        reasoning_effort: body.reasoning_effort,
        service_tier: body.service_tier,
        max_output_tokens: body.max_output_tokens,
        no_agents: body.no_agents,
        no_skills: body.no_skills,
        ..AgentConfig::default()
    };
    let event_rx = {
        let mut manager = state.run_manager.lock().await;
        match manager
            .spawn_run(options, agent, agent_config, body.prompt, body.command_id)
            .await
        {
            Ok(submission) => submission,
            Err(error) => {
                return Json(crate::error_mapping::error_envelope(&error)).into_response()
            }
        }
    };
    submission_response(event_rx)
}

pub(crate) fn submission_response(submission: RunSubmission) -> Response {
    let receiver = match submission {
        RunSubmission::Started(receiver) => receiver,
        RunSubmission::Acknowledged {
            receipt,
            run_id,
            replayed,
        } => {
            return Json(json!({"ok":true,"replayed":replayed,"run_id":run_id,"receipt":receipt}))
                .into_response();
        }
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::convert::Infallible>>(1);
    tokio::spawn(async move {
        use tokio_stream::StreamExt;
        let mut stream = BroadcastStream::new(receiver);
        while let Some(item) = stream.next().await {
            let line = match item {
                Ok(line) => line,
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(skipped)) => {
                    format!("{}\n", json!({"type":"lagged","skipped":skipped}))
                }
            };
            if tx.send(Ok(line)).await.is_err() {
                break;
            }
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(axum::body::Body::from_stream(ReceiverStream::new(rx)))
        .unwrap()
        .into_response()
}

pub async fn cancel_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Json<serde_json::Value> {
    let cleanup = state.run_manager.lock().await.cancel(&run_id);
    match cleanup {
        Some(cleanup) => match cleanup.await {
            Ok(Ok(())) => Json(json!({"ok":true})),
            Ok(Err(error)) => Json(json!({"ok":false,"code":"cleanup_failed","message":error})),
            Err(error) => {
                Json(json!({"ok":false,"code":"cleanup_failed","message":error.to_string()}))
            }
        },
        None => Json(json!({"ok":false,"code":"session_not_found","message":"run not found"})),
    }
}
