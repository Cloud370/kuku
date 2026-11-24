use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use url::Url;
use wreq::header::HeaderValue;

use crate::config::{
    ModelCapabilityDeclaration, ProtocolCapabilityDeclaration, ProviderAuth, ProviderConfig,
    ProviderFormat, SubscriptionChannel, resolve_models_url, resolve_request_url,
};
use crate::{Error, Result};

use super::capabilities::profile_for;
use super::prepare::authentication_for;

const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const CATALOG_FRESH: Duration = Duration::from_secs(120);
const CATALOG_STALE: Duration = Duration::from_secs(1800);

#[derive(Default)]
pub(super) struct CachedCatalog {
    latest: Option<(Instant, Arc<[DiscoveredModel]>)>,
}

/// One exact model-list destination and the connection to use with its results.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelListTarget {
    pub url: String,
    pub connection: ProviderConfig,
}

/// Plan same-origin model-list requests for a connection.
pub fn model_list_targets(
    connection: &ProviderConfig,
    infer_base_path: bool,
) -> Result<Vec<ModelListTarget>> {
    let mut candidates = vec![connection.clone()];
    if infer_base_path
        && connection.options.endpoint_url.is_none()
        && connection.options.models_url.is_none()
    {
        if let Ok(base) = Url::parse(&connection.base_url) {
            if base.path() == "/" && base.query().is_none() {
                let canonical = match base.host_str() {
                    Some("api.deepseek.com") => None,
                    Some("openrouter.ai" | "us.openrouter.ai" | "eu.openrouter.ai") => {
                        Some(("/api/v1", true))
                    }
                    _ if connection.format == crate::config::ProviderFormat::Anthropic => None,
                    _ => Some(("/v1", false)),
                };
                if let Some((prefix, canonical_only)) = canonical {
                    if canonical_only {
                        candidates.clear();
                    }
                    let mut candidate = connection.clone();
                    candidate.base_url =
                        format!("{}{prefix}", connection.base_url.trim_end_matches('/'));
                    candidates.insert(0, candidate);
                }
            }
        }
    }
    let mut targets = Vec::new();
    for candidate in candidates {
        let url = resolve_models_url(candidate.format, &candidate.base_url, &candidate.options)?;
        if !targets.iter().any(|item: &ModelListTarget| item.url == url) {
            targets.push(ModelListTarget {
                url,
                connection: candidate,
            });
        }
    }
    Ok(targets)
}

/// Try standard directory paths and both supported credential styles for an unclassified connection.
pub fn setup_model_list_targets(
    connection: &ProviderConfig,
    infer_base_path: bool,
) -> Result<Vec<ModelListTarget>> {
    if connection.options.subscription.is_some()
        || connection.options.models_url.is_some()
        || connection.options.endpoint_url.is_some()
    {
        return model_list_targets(connection, infer_base_path);
    }
    let mut targets = model_list_targets(connection, infer_base_path)?;
    if let Ok(base) = Url::parse(&connection.base_url) {
        if infer_base_path && base.path() == "/" && base.query().is_none() {
            let mut prefixed = connection.clone();
            prefixed.base_url = format!("{}/api/v1", connection.base_url.trim_end_matches('/'));
            for target in model_list_targets(&prefixed, false)? {
                if !targets.iter().any(|existing| existing.url == target.url) {
                    targets.push(target);
                }
            }
        }
    }
    let mut anthropic = connection.clone();
    anthropic.format = ProviderFormat::Anthropic;
    for target in model_list_targets(&anthropic, false)? {
        if !targets.iter().any(|existing| existing == &target) {
            targets.push(target);
        }
    }
    Ok(targets)
}

/// A model from the provider's model list together with its declarations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// Exact identifier to send in a model request.
    pub id: String,
    /// Optional display name supplied by the provider.
    pub name: Option<String>,
    /// Declarations scoped to this connection and model.
    pub capabilities: ModelCapabilityDeclaration,
}

/// Read a standard provider model list after the caller has confirmed its destination.
pub async fn discover_models(connection: &ProviderConfig) -> Result<Vec<DiscoveredModel>> {
    let format = connection.format;
    let options = &connection.options;
    let destination = resolve_models_url(format, &connection.base_url, options)?;
    let endpoint = resolve_request_url(
        format,
        &connection.base_url,
        options.endpoint_url.as_deref(),
    )?;
    let profile = profile_for(&endpoint, options.profile);
    let subscription = connection.options.subscription.as_ref();
    let key = match subscription {
        Some(account) => {
            account.validate_connection(format, &connection.base_url, options)?;
            super::subscription::authorization_token(account, &options.proxy).await?
        }
        None => connection.api_key.resolve()?,
    };
    let slot = cached_slot(connection, &destination, &key);
    let mut cache = if let Some(slot) = &slot {
        Some(slot.lock().await)
    } else {
        None
    };
    if let Some((at, models)) = cache.as_ref().and_then(|entry| entry.latest.as_ref()) {
        if at.elapsed() <= CATALOG_FRESH {
            return Ok(models.to_vec());
        }
    }
    let (header, value) = match authentication_for(format, profile, options) {
        ProviderAuth::Bearer => ("authorization", format!("Bearer {key}")),
        ProviderAuth::ApiKey => ("x-api-key", key),
    };
    let mut credential = HeaderValue::from_str(&value)
        .map_err(|_| discovery_error("credential is not a valid HTTP header value"))?;
    credential.set_sensitive(true);
    let mut request = super::http_client::api_client(&options.proxy)?
        .get(&destination)
        .redirect(wreq::redirect::Policy::none())
        .timeout(DISCOVERY_TIMEOUT)
        .header(header, credential);
    if format == crate::config::ProviderFormat::Anthropic {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = request.send().await.map_err(|error| {
        discovery_error(if error.is_timeout() {
            "model-list request timed out"
        } else {
            "could not reach the model-list endpoint"
        })
    })?;
    if !response.status().is_success() {
        return Err(discovery_error(&format!(
            "model-list endpoint returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let models = read_model_list(response, subscription.map(|account| account.channel())).await?;
    if let Some(cache) = &mut cache {
        cache.latest = Some((Instant::now(), Arc::from(models.clone())));
    }
    Ok(models)
}

fn cached_slot(
    connection: &ProviderConfig,
    destination: &str,
    key: &str,
) -> Option<Arc<Mutex<CachedCatalog>>> {
    let url = Url::parse(destination).ok()?;
    (url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some("openrouter.ai" | "us.openrouter.ai" | "eu.openrouter.ai")
        )
        && url.path() == "/api/v1/models")
        .then(|| {
            super::http_client::catalog_slot(
                destination.into(),
                connection.options.proxy.clone(),
                key,
            )
        })
}

pub(super) fn cached_models(target: &ModelListTarget) -> Option<Arc<[DiscoveredModel]>> {
    let key = target.connection.api_key.resolve().ok()?;
    let slot = cached_slot(&target.connection, &target.url, &key)?;
    let cache = slot.try_lock().ok()?;
    let (at, models) = cache.latest.as_ref()?;
    (at.elapsed() <= CATALOG_STALE).then(|| Arc::clone(models))
}

async fn read_model_list(
    response: wreq::Response,
    subscription: Option<SubscriptionChannel>,
) -> Result<Vec<DiscoveredModel>> {
    let stream = response.bytes_stream();
    tokio::pin!(stream);
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| discovery_error("could not read the model list"))?;
        if bytes.len() + chunk.len() > MAX_LIST_BYTES {
            return Err(discovery_error("model list exceeds the response limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    match subscription {
        Some(SubscriptionChannel::ChatGpt) => parse_chatgpt_models(&bytes),
        None => parse_model_list(&bytes),
    }
}

fn parse_chatgpt_models(bytes: &[u8]) -> Result<Vec<DiscoveredModel>> {
    #[derive(Deserialize)]
    struct Catalog {
        models: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        slug: String,
        display_name: Option<String>,
        visibility: Option<String>,
        service_tiers: Option<Vec<CatalogValue>>,
        supported_reasoning_levels: Option<Vec<CatalogValue>>,
        default_reasoning_level: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CatalogValue {
        Id(String),
        ServiceTier { id: String },
        ReasoningLevel { effort: String },
        Unknown(serde::de::IgnoredAny),
    }

    fn declared_values(values: Option<Vec<CatalogValue>>) -> Option<Vec<String>> {
        let values = values?;
        let empty = values.is_empty();
        let ids: Vec<_> = values
            .into_iter()
            .filter_map(|value| match value {
                CatalogValue::Id(id) | CatalogValue::ServiceTier { id } => Some(id),
                CatalogValue::ReasoningLevel { effort } => Some(effort),
                CatalogValue::Unknown(_) => None,
            })
            .collect();
        (empty || !ids.is_empty()).then_some(ids)
    }

    let catalog: Catalog = serde_json::from_slice(bytes)
        .map_err(|_| discovery_error("unexpected ChatGPT model catalog format"))?;
    let mut seen = HashSet::new();
    Ok(catalog
        .models
        .into_iter()
        .filter(|model| {
            model.visibility.as_deref() == Some("list")
                && valid_model_id(&model.slug)
                && seen.insert(model.slug.clone())
        })
        .map(|model| DiscoveredModel {
            id: model.slug,
            name: model.display_name,
            capabilities: ModelCapabilityDeclaration {
                service_tiers: declared_values(model.service_tiers),
                reasoning_efforts: declared_values(model.supported_reasoning_levels),
                reasoning_default: model.default_reasoning_level,
                ..ModelCapabilityDeclaration::default()
            },
        })
        .collect())
}

#[derive(Deserialize)]
struct List {
    data: Vec<ListModel>,
}

#[derive(Deserialize)]
struct ListModel {
    id: String,
    #[serde(alias = "display_name")]
    name: Option<String>,
    effort: Option<Effort>,
    reasoning: Option<Reasoning>,
    reasoning_efforts: Option<Vec<String>>,
    service_tiers: Option<Vec<String>>,
    input_modalities: Option<Vec<String>>,
    output_modalities: Option<Vec<String>>,
    architecture: Option<Architecture>,
    endpoints: Option<Vec<String>>,
    supported_endpoint_types: Option<Vec<String>>,
    api_capabilities: Option<BTreeMap<String, ProtocolCapabilityDeclaration>>,
}

#[derive(Deserialize)]
struct Effort {
    supported_levels: Option<Vec<String>>,
    default_level: Option<String>,
}

#[derive(Deserialize)]
struct Reasoning {
    supported_efforts: Option<Vec<String>>,
    default_effort: Option<String>,
}

#[derive(Default, Deserialize)]
struct Architecture {
    input_modalities: Option<Vec<String>>,
    output_modalities: Option<Vec<String>>,
}

fn parse_model_list(bytes: &[u8]) -> Result<Vec<DiscoveredModel>> {
    let list = serde_json::from_slice::<List>(bytes)
        .map_err(|_| discovery_error("endpoint did not return a standard model list"))?;
    let mut models = Vec::with_capacity(list.data.len());
    let mut seen = HashSet::new();
    for model in list.data {
        if !valid_model_id(&model.id) {
            continue;
        }
        if !seen.insert(model.id.clone()) {
            continue;
        }
        let architecture = model.architecture.unwrap_or_default();
        let reasoning = model.reasoning.unwrap_or(Reasoning {
            supported_efforts: None,
            default_effort: None,
        });
        let effort = model.effort.unwrap_or(Effort {
            supported_levels: None,
            default_level: None,
        });
        let mut interfaces = model.endpoints.unwrap_or_default();
        for value in model.supported_endpoint_types.into_iter().flatten() {
            if !interfaces.contains(&value) {
                interfaces.push(value);
            }
        }
        models.push(DiscoveredModel {
            id: model.id,
            name: model.name,
            capabilities: ModelCapabilityDeclaration {
                reasoning_efforts: effort
                    .supported_levels
                    .or(reasoning.supported_efforts)
                    .or(model.reasoning_efforts),
                reasoning_default: effort.default_level.or(reasoning.default_effort),
                service_tiers: model.service_tiers,
                input_modalities: model.input_modalities.or(architecture.input_modalities),
                output_modalities: model.output_modalities.or(architecture.output_modalities),
                interfaces: (!interfaces.is_empty()).then_some(interfaces),
                protocols: model.api_capabilities.unwrap_or_default(),
            },
        });
    }
    Ok(models)
}

fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('$')
        && !id
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn discovery_error(message: &str) -> Error {
    Error::InvalidArgument(format!("Model discovery: {message}"))
}

#[cfg(test)]
mod tests {
    use super::{parse_chatgpt_models, parse_model_list};

    #[test]
    fn chatgpt_models_follow_the_accounts_order_and_visibility() {
        let models = parse_chatgpt_models(
            br#"{"models":[
            {"slug":"second","display_name":"Second","visibility":"list","service_tiers":[
                {"id":"priority","name":"Fast","description":"Faster processing"},
                {"id":"ultrafast","name":"Ultrafast","description":"Lowest latency"},
                {"name":"Unrecognized"}, "default"
            ], "supported_reasoning_levels":[{"effort":"low","description":"Low"},
                {"effort":"high","description":"High"},{"name":"Unrecognized"}],
               "default_reasoning_level":"low"},
            {"slug":"hidden","visibility":"private"},
            {"slug":"first","display_name":"First","visibility":"list"},
            {"slug":"first","display_name":"Ignore duplicate","visibility":"list"}
        ]}"#,
        )
        .unwrap();
        assert_eq!(
            vec![("second", Some("Second")), ("first", Some("First"))],
            models
                .iter()
                .map(|model| (model.id.as_str(), model.name.as_deref()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            Some(vec![
                "priority".into(),
                "ultrafast".into(),
                "default".into()
            ]),
            models[0].capabilities.service_tiers
        );
        assert_eq!(None, models[1].capabilities.service_tiers);
        assert_eq!(
            Some(vec!["low".into(), "high".into()]),
            models[0].capabilities.reasoning_efforts
        );
        assert_eq!(
            Some("low"),
            models[0].capabilities.reasoning_default.as_deref()
        );
        assert_eq!(None, models[1].capabilities.reasoning_efforts);
    }

    #[test]
    fn provider_model_lists_keep_omissions_and_protocol_scope() {
        let models = parse_model_list(br#"{"data":[
            {"id":"ids-only"},
            {"id":"deepseek-flash","effort":{"supported_levels":["low","high"],"default_level":"high"},
            "input_modalities":["text","image"],"output_modalities":["text"],
            "api_capabilities":{"anthropic_messages":{"system_prompt_update":"in-history"}}},
            {"id":"router/model","reasoning":{"supported_efforts":["none","max"],"default_effort":"max"},
            "architecture":{"input_modalities":["text"],"output_modalities":["text","image"]},
            "endpoints":["/api/v1/responses"]},
            {"id":"ids-only","reasoning_efforts":["low"]},
            {"id":"declared","reasoning_efforts":["low","high"],"supported_endpoint_types":["openai"]}
        ]}"#).unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["ids-only", "deepseek-flash", "router/model", "declared"]
        );
        assert_eq!(None, models[0].capabilities.reasoning_efforts);
        assert_eq!(None, models[0].capabilities.output_modalities);
        assert_eq!(
            Some("high"),
            models[1].capabilities.reasoning_default.as_deref()
        );
        assert_eq!(
            Some("in-history"),
            models[1].capabilities.protocols["anthropic_messages"]
                .system_prompt_update
                .as_deref()
        );
        assert_eq!(
            Some("max"),
            models[2].capabilities.reasoning_default.as_deref()
        );
        assert_eq!(
            Some(vec!["/api/v1/responses".into()]),
            models[2].capabilities.interfaces
        );
        assert_eq!(
            Some(vec!["low".into(), "high".into()]),
            models[3].capabilities.reasoning_efforts
        );
        assert_eq!(
            Some(vec!["openai".into()]),
            models[3].capabilities.interfaces
        );
    }
}
