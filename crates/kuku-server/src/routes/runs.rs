use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

use crate::AppState;

#[derive(Deserialize)]
pub struct RunRequest {
    pub prompt: String,
    pub workspace: PathBuf,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
}

pub async fn create_run(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RunRequest>,
) -> Response {
    if !body.workspace.exists() {
        return Json(
            json!({"ok": false, "code": "invalid_request", "message": "workspace does not exist"}),
        )
        .into_response();
    }

    let config = state.config.load();

    let mut query = kuku::Query::new(body.prompt)
        .workspace(body.workspace)
        .config((**config).clone());

    if let Some(sid) = body.session_id {
        query = query.session(sid);
    }
    if let Some(tier) = body.tier {
        query = query.tier(tier);
    }

    let (_run_id, event_rx) = {
        let mut mgr = state.run_manager.lock().await;
        match mgr.spawn_run(query).await {
            Ok(pair) => pair,
            Err(e) => {
                let envelope = crate::error_mapping::error_envelope(&e);
                return Json(envelope).into_response();
            }
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::convert::Infallible>>(1);
    tokio::spawn(async move {
        use tokio_stream::StreamExt;
        let mut bstream = BroadcastStream::new(event_rx);
        while let Some(item) = bstream.next().await {
            if let Ok(line) = item {
                if tx.send(Ok(line)).await.is_err() {
                    break;
                }
            }
        }
    });
    let stream = ReceiverStream::new(rx);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
        .into_response()
}

pub async fn cancel_run(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Json<serde_json::Value> {
    let jh = {
        let mgr = state.run_manager.lock().await;
        mgr.cancel(&run_id)
    };
    if let Some(jh) = jh {
        let _ = jh.await;
        Json(json!({"ok": true}))
    } else {
        Json(json!({"ok": false, "code": "session_not_found", "message": "run not found"}))
    }
}
