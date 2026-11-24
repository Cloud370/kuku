use std::time::Duration;

use kuku::adapter::{
    ModelTrial, inspect_connection_model, inspect_model, trial_model_with_route_request,
};
use kuku::config::{CustomServiceTier, RequestFields};
use kuku_client::{Connection, ConnectionOptions};
use toml_edit::{InlineTable, Item, TableLike, Value};

use crate::app::speed::SpeedTarget;
use crate::local_settings::{active_service, ensure_same_service};

use super::{config, editor::active_settings};

#[derive(Debug)]
pub(crate) enum SpeedWork {
    Probe(String),
    Save(String, CustomServiceTier),
}

pub(crate) enum SpeedResult {
    Confirmed(String),
    Unconfirmed(String),
    Other(String),
    Saved(String),
}

pub(crate) fn valid_tier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || ch == ',')
}

pub(crate) async fn custom_service_tier(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    target: &SpeedTarget,
    work: SpeedWork,
) -> Result<SpeedResult, String> {
    let (service, settings) = active_settings(options, connection).await?;
    let route =
        inspect_model(&settings.resolved, &target.config).map_err(|error| error.to_string())?;
    if !target.same_route(&route) {
        return Err("The selected Agent's model changed. Reopen /speed.".into());
    }
    let provider = settings
        .resolved
        .provider(&route.connection)
        .ok_or("Choose a configured connection before adding a service tier.")?;
    match work {
        SpeedWork::Probe(tier) => {
            let tier_name = target
                .config
                .tier
                .as_deref()
                .unwrap_or_else(|| settings.resolved.default_tier());
            let request = settings
                .resolved
                .tier(tier_name)
                .filter(|model| model.provider == route.connection && model.model == route.model)
                .and_then(|model| model.request.as_ref());
            probe(
                provider,
                &route.model,
                route.settings.reasoning_effort,
                tier,
                request,
            )
            .await
        }
        SpeedWork::Save(tier, evidence) => {
            let mut document = settings.document.clone();
            let fields = config::entry(&mut document, "provider", &route.connection)
                .map_err(|error| error.to_string())?;
            let capabilities = child(fields, "model_capabilities")?;
            let model = child(capabilities, &route.model)?;
            let custom = child(model, "custom_service_tiers")?;
            let evidence_name = match evidence {
                CustomServiceTier::Unverified => "unverified",
                CustomServiceTier::Confirmed => "confirmed",
            };
            if custom.get(&tier).and_then(Item::as_str) != Some("confirmed") {
                custom.insert(&tier, toml_edit::value(evidence_name));
            }

            let current = active_service(options, connection).await?;
            ensure_same_service(&service, &current)?;
            let routes: Vec<_> = settings
                .resolved
                .tiers
                .iter()
                .filter_map(|(name, model)| {
                    (model.provider == route.connection && model.model == route.model)
                        .then_some(name.as_str())
                })
                .collect();
            config::save_model_document(&settings, &document, &routes)?;
            wait_for_tier(connection, target, &tier).await?;
            Ok(SpeedResult::Saved(tier))
        }
    }
}

async fn probe(
    provider: &kuku::config::ProviderConfig,
    model: &str,
    effort: Option<String>,
    tier: String,
    route_request: Option<&RequestFields>,
) -> Result<SpeedResult, String> {
    let mut candidate = provider.clone();
    candidate
        .options
        .model_capabilities
        .entry(model.into())
        .or_default()
        .custom_service_tiers
        .insert(tier.clone(), CustomServiceTier::Unverified);
    let capabilities =
        inspect_connection_model(&candidate, model).map_err(|error| error.to_string())?;
    let result = trial_model_with_route_request(
        &candidate,
        model,
        ModelTrial::Text {
            effort,
            service_tier: Some(tier.clone()),
        },
        route_request,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(probe_result(
        &capabilities,
        tier,
        result
            .provider_report
            .and_then(|report| report.service_tier),
    ))
}

fn child<'a>(parent: &'a mut dyn TableLike, name: &str) -> Result<&'a mut dyn TableLike, String> {
    if !parent.contains_key(name) {
        parent.insert(name, Item::Value(Value::InlineTable(InlineTable::new())));
    }
    parent
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| format!("The configuration entry '{name}' cannot be edited."))
}

fn probe_result(
    capabilities: &kuku::ProviderCapabilities,
    tier: String,
    reported: Option<String>,
) -> SpeedResult {
    match reported {
        Some(actual) if capabilities.equivalent_service_tiers(&tier, &actual) => {
            SpeedResult::Confirmed(tier)
        }
        Some(actual) => SpeedResult::Other(actual),
        None => SpeedResult::Unconfirmed(tier),
    }
}

async fn wait_for_tier(
    connection: Option<&Connection>,
    target: &SpeedTarget,
    tier: &str,
) -> Result<(), String> {
    let Some(connection) = connection else {
        return Err("Saved the service tier. Reconnect before applying it.".into());
    };
    let mut config = target.config.clone();
    config.service_tier = Some(tier.into());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if connection
                .client
                .resolve_model(&config)
                .await
                .is_ok_and(|model| {
                    target.same_route(&model)
                        && model.capabilities.custom_service_tiers.contains_key(tier)
                })
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(125)).await;
        }
    })
    .await
    .map_err(|_| "Saved the service tier. The service is still loading it.".to_owned())
}

#[cfg(test)]
mod tests {
    use httpmock::{Method::POST, MockServer};
    use kuku::config::{
        ApiKey, ProviderConfig, ProviderFormat, ProviderOptions, ProviderProfile, ProviderProxy,
        RequestFields,
    };
    use serde_json::json;

    use super::{SpeedResult, probe, probe_result};

    #[tokio::test]
    async fn custom_tier_probe_uses_configured_proxy_and_selected_route_with_synthetic_input() {
        let proxy = MockServer::start_async().await;
        let request = proxy.mock_async(|when, then| {
            when.method(POST)
                .path("/v1/responses")
                .header("host", "test-unchosen.invalid")
                .header("authorization", "Bearer fixture-key")
                .json_body_partial(r#"{"model":"fixture","service_tier":"turbo","provider":{"only":["route-upstream"]},"input":[{"role":"user","content":[{"type":"input_text","text":"What is 17 times 23 minus 14? Reply with the number only."}]}]}"#);
            then.status(200).json_body(json!({
                "model":"fixture", "status":"completed", "service_tier":"turbo",
                "output":[{"type":"message","content":[{"type":"output_text","text":"377"}]}]
            }));
        }).await;
        let connection = ProviderConfig {
            format: ProviderFormat::OpenAiResponses,
            base_url: "http://test-unchosen.invalid/v1".into(),
            api_key: ApiKey::Plaintext("fixture-key".into()),
            options: ProviderOptions {
                profile: ProviderProfile::OpenRouter,
                request: Some(
                    serde_json::from_value(
                        json!({"body":{"provider":{"only":["connection-upstream"]}}}),
                    )
                    .unwrap(),
                ),
                proxy: ProviderProxy::Explicit {
                    url: proxy.base_url(),
                    auth: None,
                },
                ..Default::default()
            },
        };
        let route: RequestFields =
            serde_json::from_value(json!({"body":{"provider":{"only":["route-upstream"]}}}))
                .unwrap();
        assert!(
            matches!(probe(&connection, "fixture", None, "turbo".into(), Some(&route)).await.unwrap(), SpeedResult::Confirmed(tier) if tier == "turbo")
        );
        request.assert_hits_async(1).await;
        assert!(connection.options.model_capabilities.is_empty());
    }

    #[test]
    fn probe_requires_an_equivalent_upstream_tier_to_confirm() {
        let capabilities = kuku::ProviderCapabilities {
            profile: ProviderProfile::OpenRouter,
            ..Default::default()
        };
        assert!(
            matches!(probe_result(&capabilities, "fast".into(), Some("priority".into())), SpeedResult::Confirmed(tier) if tier == "fast")
        );
        assert!(
            matches!(probe_result(&capabilities, "ultrafast".into(), Some("default".into())), SpeedResult::Other(tier) if tier == "default")
        );
        assert!(
            matches!(probe_result(&capabilities, "turbo".into(), None), SpeedResult::Unconfirmed(tier) if tier == "turbo")
        );
    }
}
