//! Public liveness and protocol revisions for the HTTP service.

use axum::Json;
use serde_json::json;

use crate::BUILD_VERSION;
use crate::service::{MANAGEMENT_VERSION, SESSION_PROTOCOL_VERSION};

/// Return liveness, build version and protocol revisions.
pub async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "version": BUILD_VERSION,
        "protocol_version": SESSION_PROTOCOL_VERSION,
        "management_version": MANAGEMENT_VERSION,
    }))
}
