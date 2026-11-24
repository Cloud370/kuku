use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use kuku::adapter::{inspect_model, subscription_account_info, subscription_display_label};
use kuku::session::AgentConfig;
use serde_json::{Value, json};

use super::Params;
use crate::AppState;
use crate::error_mapping::{ApiError, Result};

pub(super) async fn list(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.config.load_full();
    let labels = if config
        .providers
        .values()
        .any(|provider| provider.options.subscription.is_some())
    {
        let config = Arc::clone(&config);
        tokio::task::spawn_blocking(move || {
            config
                .providers
                .iter()
                .filter_map(|(name, provider)| {
                    let account = provider.options.subscription.as_ref()?;
                    let info = subscription_account_info(account).ok();
                    Some((
                        name.clone(),
                        subscription_display_label(account, info.as_ref()),
                    ))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .await
        .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    let connections = config
        .providers
        .iter()
        .map(|(name, provider)| {
            let mut connection = json!({"name":name,"protocol":provider.format});
            if let Some(label) = labels.get(name) {
                connection["subscription_label"] = json!(label);
            }
            connection
        })
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
