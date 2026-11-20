use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use kuku::adapter::inspect_model;
use kuku::config::{parse_config_text, ModelEntry};
use kuku::session::AgentConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{failure, ServiceState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelDefaultPlanRequest {
    connection: String,
    model: String,
    config_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ModelDefaultPlan {
    tier: String,
    overwrite_user_default: bool,
    config_sha256: String,
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
    let (Some(_identity), Some(config_path)) = (&service.identity, &service.config_path) else {
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
    let path = config_path.clone();
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
    let config_sha256 = format!("{:x}", Sha256::digest(bytes));
    if config_sha256 != request.config_sha256 {
        return Err(PlanFailure::ConfigChanged);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| PlanFailure::ConfigInvalid)?;
    let file = parse_config_text(text).map_err(|_| PlanFailure::ConfigInvalid)?;
    let config = file.resolve().map_err(|_| PlanFailure::ConfigInvalid)?;

    let mut matching = config
        .tiers
        .iter()
        .filter(|(_, tier)| tier.provider == request.connection && tier.model == request.model)
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if let Some(index) = matching
        .iter()
        .position(|name| name == &config.default_tier)
    {
        let default = matching.remove(index);
        matching.insert(0, default);
    }

    if !matching.is_empty() {
        for tier in matching {
            if inspect_model(
                &config,
                &AgentConfig {
                    tier: Some(tier.clone()),
                    ..AgentConfig::default()
                },
            )
            .is_ok_and(|selection| {
                selection.connection == request.connection && selection.model == request.model
            }) {
                return Ok(ModelDefaultPlan {
                    tier,
                    overwrite_user_default: false,
                    config_sha256,
                });
            }
        }
        return Err(PlanFailure::RouteUnusable);
    }

    let mut fallback = file;
    fallback.model.insert(
        "user-default".into(),
        ModelEntry {
            provider: request.connection.clone(),
            model: request.model.clone(),
            think: None,
            reasoning_effort: None,
            service_tier: None,
            context_window: None,
            max_output_tokens: None,
            purpose: Some("Saved by /model".into()),
        },
    );
    fallback.default_model = Some("user-default".into());
    let fallback = fallback.resolve().map_err(|_| PlanFailure::RouteUnusable)?;
    if !inspect_model(
        &fallback,
        &AgentConfig {
            tier: Some("user-default".into()),
            ..AgentConfig::default()
        },
    )
    .is_ok_and(|selection| {
        selection.connection == request.connection && selection.model == request.model
    }) {
        return Err(PlanFailure::RouteUnusable);
    }
    Ok(ModelDefaultPlan {
        tier: "user-default".into(),
        overwrite_user_default: true,
        config_sha256,
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
    use std::sync::Arc;

    use axum::extract::{Query, State};
    use axum::http::{header, StatusCode};
    use sha2::{Digest, Sha256};

    use super::{plan, resolve_plan, ModelDefaultPlanRequest, PlanFailure, ServiceState};

    const CONFIG: &str = r#"
default_model = "balanced"

[provider.openai]
format = "openai-chat"
base_url = "https://api.openai.com/v1"
api_key = "test-key"
profile = "openai"
service_tiers = ["auto", "priority"]
reasoning_efforts = ["low", "medium", "high"]

[model.strong]
provider = "openai"
model = "gpt-5"
think = "off"

[model.balanced]
provider = "openai"
model = "gpt-4"
think = "off"

[model.light]
provider = "openai"
model = "gpt-3.5"
think = "off"
"#;

    fn request(bytes: &[u8], model: &str) -> ModelDefaultPlanRequest {
        ModelDefaultPlanRequest {
            connection: "openai".into(),
            model: model.into(),
            config_sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }

    #[test]
    fn uses_the_service_environment_and_prefers_a_usable_matching_default() {
        const MODEL: &str = "KUKU_MODEL_DEFAULT_SERVICE_MODEL";
        let old_model = std::env::var_os(MODEL);
        std::env::set_var(MODEL, "server-model");
        let input = CONFIG.replace(
            "model = \"gpt-4\"",
            "model = \"$KUKU_MODEL_DEFAULT_SERVICE_MODEL\"",
        );
        let alternate = format!("{input}\n[model.alternate]\nprovider = \"openai\"\nmodel = \"server-model\"\nthink = \"off\"\n");
        let plan = resolve_plan(
            alternate.as_bytes(),
            &request(alternate.as_bytes(), "server-model"),
        )
        .unwrap();
        if let Some(value) = old_model {
            std::env::set_var(MODEL, value);
        } else {
            std::env::remove_var(MODEL);
        }
        assert_eq!("balanced", plan.tier);
        assert!(!plan.overwrite_user_default);
    }

    #[test]
    fn falls_back_to_another_usable_matching_tier_before_user_default() {
        let input = CONFIG.replace(
            "model = \"gpt-4\"\nthink = \"off\"",
            "model = \"gpt-4\"\nthink = \"off\"\nservice_tier = \"invalid\"",
        )
            + "\n[model.alternate]\nprovider = \"openai\"\nmodel = \"gpt-4\"\nthink = \"off\"\n";
        let plan = resolve_plan(input.as_bytes(), &request(input.as_bytes(), "gpt-4")).unwrap();
        assert_eq!("alternate", plan.tier);
        assert!(!plan.overwrite_user_default);
    }

    #[test]
    fn rejects_when_all_matching_tiers_are_unusable() {
        let input = CONFIG
            .replace("model = \"gpt-4\"\nthink = \"off\"", "model = \"gpt-4\"\nthink = \"off\"\nservice_tier = \"invalid\"")
            + "\n[model.alternate]\nprovider = \"openai\"\nmodel = \"gpt-4\"\nthink = \"off\"\nservice_tier = \"invalid\"\n";
        assert!(matches!(
            resolve_plan(input.as_bytes(), &request(input.as_bytes(), "gpt-4")),
            Err(PlanFailure::RouteUnusable)
        ));
    }

    #[test]
    fn fallback_validation_includes_provider_options() {
        let invalid = CONFIG.replace(
            "profile = \"openai\"",
            "profile = \"compatible\"\nrouting = { order = [\"fast\"] }",
        );
        assert!(matches!(
            resolve_plan(
                invalid.as_bytes(),
                &request(invalid.as_bytes(), "new-model")
            ),
            Err(PlanFailure::RouteUnusable)
        ));
        let valid = CONFIG
            .replace(
                "base_url = \"https://api.openai.com/v1\"",
                "base_url = \"https://openrouter.ai/api/v1\"",
            )
            .replace(
                "profile = \"openai\"",
                "profile = \"openrouter\"\nrouting = { order = [\"fast\"] }",
            );
        let plan = resolve_plan(valid.as_bytes(), &request(valid.as_bytes(), "new-model")).unwrap();
        assert_eq!("user-default", plan.tier);
        assert!(plan.overwrite_user_default);
    }

    #[test]
    fn changed_config_hash_is_rejected() {
        let bytes = CONFIG.as_bytes();
        let mut request = request(bytes, "gpt-4");
        request.config_sha256 = "0".repeat(64);
        assert!(matches!(
            resolve_plan(bytes, &request),
            Err(PlanFailure::ConfigChanged)
        ));
    }

    #[tokio::test]
    async fn unmanaged_service_cannot_validate_a_model_default_plan() {
        use arc_swap::ArcSwap;
        use kuku::config::Config;
        use tokio::sync::Mutex;

        let app = Arc::new(crate::AppState {
            run_manager: Mutex::new(crate::run_manager::RunManager::new(1)),
            config: Arc::new(ArcSwap::from_pointee(Config {
                tiers: Default::default(),
                providers: Default::default(),
                default_tier: String::new(),
                discovery: Default::default(),
                handoff: Default::default(),
                logs: Default::default(),
                plugin: Default::default(),
                update: Default::default(),
            })),
            password: None,
            kuku_home: Default::default(),
            authorizer: Arc::new(kuku::permission::TrustedLocal),
        });
        let service = Arc::new(ServiceState::new(
            app,
            None,
            Some("/untrusted/path-is-not-used".into()),
            None,
        ));
        let response = plan(
            State(service),
            Query(ModelDefaultPlanRequest {
                connection: "openai".into(),
                model: "gpt-4".into(),
                config_sha256: "0".repeat(64),
            }),
        )
        .await;
        assert_eq!(StatusCode::CONFLICT, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    }
}
