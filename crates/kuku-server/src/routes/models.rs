use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use kuku::adapter::inspect_model;
use kuku::session::AgentConfig;
use serde_json::{json, Value};

use super::Params;
use crate::error_mapping::{ApiError, Result};
use crate::AppState;

pub(super) async fn list(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.config.load();
    let connections = config
        .providers
        .iter()
        .map(|(name, provider)| json!({"name":name,"protocol":provider.format}))
        .collect::<Vec<_>>();
    let tiers = config
        .tiers
        .iter()
        .map(|(name, tier)| {
            let selection = AgentConfig {
                tier: Some(name.clone()),
                ..AgentConfig::default()
            };
            let mut value = json!({
                "name":name,
                "connection":tier.provider,
                "model":tier.model,
                "purpose":tier.purpose,
            });
            match inspect_model(&config, &selection) {
                Ok(resolved) => {
                    value["settings"] = json!(resolved.settings);
                    value["output"] = json!(resolved.output);
                    value["request_url"] = json!(resolved.request_url);
                    value["capabilities"] = json!(resolved.capabilities);
                }
                Err(error) => value["error"] = ApiError::from(error).value(),
            }
            value
        })
        .collect::<Vec<_>>();
    Json(
        json!({"ok":true,"default_tier":config.default_tier,"connections":connections,"tiers":tiers}),
    )
}

pub(super) async fn resolve(
    State(state): State<Arc<AppState>>,
    Params(selection): Params<AgentConfig>,
) -> Result<Json<Value>> {
    let selection = inspect_model(&state.config.load(), &selection)?;
    Ok(Json(json!({"ok":true,"selection":selection})))
}
