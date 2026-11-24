use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use kuku::adapter::inspect_model;
use kuku::config::{ModelRef, config_sha256, parse_config_text};
use kuku::session::AgentConfig;
use serde::{Deserialize, Serialize};

use super::{ServiceState, failure};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelDefaultPlanRequest {
    connection: String,
    model: String,
    config_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ModelDefaultPlan {
    config_sha256: String,
    already_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    register_previous_default: Option<ModelRef>,
}

#[derive(Debug)]
enum PlanFailure {
    ConfigChanged,
    ConfigInvalid,
    RouteUnusable,
}

pub(super) async fn plan(
    State(service): State<Arc<ServiceState>>,
    Query(request): Query<ModelDefaultPlanRequest>,
) -> Response {
    let (Some(_identity), Some(config_source)) = (&service.identity, &service.config_source) else {
        return no_store(failure(
            StatusCode::CONFLICT,
            "managed_service_required",
            "model default plans require the managed service configuration",
        ));
    };
    if request.connection.trim().is_empty() || request.model.trim().is_empty() {
        return no_store(failure(
            StatusCode::BAD_REQUEST,
            "invalid_model_default_plan",
            "a resolved connection and model are required",
        ));
    }
    let path = config_source.path.clone();
    let result = tokio::task::spawn_blocking(move || resolve_file_plan(&path, &request)).await;
    match result {
        Ok(Ok(plan)) => no_store(Json(plan).into_response()),
        Ok(Err(PlanFailure::ConfigChanged)) => no_store(failure(
            StatusCode::CONFLICT,
            "config_changed",
            "the active config changed; reopen model settings",
        )),
        Ok(Err(PlanFailure::ConfigInvalid)) => no_store(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "config_invalid",
            "the active config cannot be used to validate a model default",
        )),
        Ok(Err(PlanFailure::RouteUnusable)) => no_store(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "model_default_unusable",
            "the active service cannot use this model route as a default",
        )),
        Err(_) => no_store(failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "model_default_unavailable",
            "the active config could not be checked",
        )),
    }
}

fn resolve_file_plan(
    path: &Path,
    request: &ModelDefaultPlanRequest,
) -> Result<ModelDefaultPlan, PlanFailure> {
    let bytes = fs::read(path).map_err(|_| PlanFailure::ConfigInvalid)?;
    resolve_plan(&bytes, request)
}

fn resolve_plan(
    bytes: &[u8],
    request: &ModelDefaultPlanRequest,
) -> Result<ModelDefaultPlan, PlanFailure> {
    let config_sha256 = config_sha256(bytes);
    if config_sha256 != request.config_sha256 {
        return Err(PlanFailure::ConfigChanged);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| PlanFailure::ConfigInvalid)?;
    let file = parse_config_text(text).map_err(|_| PlanFailure::ConfigInvalid)?;
    let config = file.resolve().map_err(|_| PlanFailure::ConfigInvalid)?;

    let route = ModelRef::new(&request.connection, &request.model);
    if config.model(&route).is_none() {
        return Err(PlanFailure::RouteUnusable);
    }
    let current = config.default_model();
    let already_default = &route == current;
    let register_previous_default = if !already_default
        && !file
            .model
            .get(&current.connection)
            .is_some_and(|models| models.contains_key(&current.model))
    {
        #[derive(Deserialize)]
        struct RawDefault {
            default_model: ModelRef,
        }
        let raw = toml::from_str::<RawDefault>(text).map_err(|_| PlanFailure::ConfigInvalid)?;
        Some(ModelRef::new(&current.connection, raw.default_model.model))
    } else {
        None
    };
    inspect_model(
        &config,
        &AgentConfig {
            connection: Some(route.connection),
            model: Some(route.model),
            ..AgentConfig::default()
        },
    )
    .map_err(|_| PlanFailure::RouteUnusable)?;
    Ok(ModelDefaultPlan {
        config_sha256,
        already_default,
        register_previous_default,
    })
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use kuku::config::config_sha256;

    use super::{ModelDefaultPlanRequest, PlanFailure, resolve_plan};

    const CONFIG: &str = r#"
default_model = { connection = "work", model = "chat" }

[provider.work]
format = "openai-chat"
base_url = "https://example.org/v1"
api_key = "test-key"

[provider.other]
format = "openai-chat"
base_url = "https://other.example.org/v1"
api_key = "test-key"

[model.other."chat"]
max_output_tokens = 512
"#;

    fn request(connection: &str, model: &str) -> ModelDefaultPlanRequest {
        ModelDefaultPlanRequest {
            connection: connection.into(),
            model: model.into(),
            config_sha256: config_sha256(CONFIG.as_bytes()),
        }
    }

    #[test]
    fn accepts_only_the_exact_registered_pair_including_the_implicit_default() {
        for connection in ["work", "other"] {
            let plan = resolve_plan(CONFIG.as_bytes(), &request(connection, "chat")).unwrap();
            assert_eq!(config_sha256(CONFIG.as_bytes()), plan.config_sha256);
            assert_eq!(connection == "work", plan.already_default);
            assert_eq!(
                (connection == "other").then(|| kuku::config::ModelRef::new("work", "chat")),
                plan.register_previous_default,
            );
        }
        assert!(matches!(
            resolve_plan(CONFIG.as_bytes(), &request("work", "unregistered")),
            Err(PlanFailure::RouteUnusable)
        ));
    }

    #[test]
    fn rejects_file_changes_since_the_client_snapshot() {
        let mut request = request("work", "chat");
        request.config_sha256 = "0".repeat(64);
        assert!(matches!(
            resolve_plan(CONFIG.as_bytes(), &request),
            Err(PlanFailure::ConfigChanged)
        ));
    }
}
