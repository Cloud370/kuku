use std::sync::Arc;

use axum::Json;
use serde_json::json;

pub async fn health(state: Arc<crate::AppState>) -> Json<serde_json::Value> {
    let status = if state.platform.bootstrap.status().await.complete {
        "ok"
    } else {
        "init_required"
    };
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "status": status,
    }))
}
