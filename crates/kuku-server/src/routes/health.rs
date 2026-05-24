use axum::Json;
use serde_json::json;

pub async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "workspace": std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy(),
    }))
}
