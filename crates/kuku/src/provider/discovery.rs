use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use wreq::header::HeaderValue;

use crate::config::{
    resolve_models_url, resolve_request_url, ModelCapabilityDeclaration,
    ProtocolCapabilityDeclaration, ProviderAuth, ProviderConfig,
};
use crate::{Error, Result};

use super::capabilities::profile_for;
use super::prepare::authentication_for;

const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

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
    let key = connection.api_key.resolve()?;
    let (header, value) = match authentication_for(format, profile, options) {
        ProviderAuth::Bearer => ("authorization", format!("Bearer {key}")),
        ProviderAuth::ApiKey => ("x-api-key", key),
    };
    let mut credential = HeaderValue::from_str(&value)
        .map_err(|_| discovery_error("credential is not a valid HTTP header value"))?;
    credential.set_sensitive(true);
    let mut request = super::http_client::api_client()
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
    read_model_list(response).await
}

async fn read_model_list(response: wreq::Response) -> Result<Vec<DiscoveredModel>> {
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
    parse_model_list(&bytes)
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
    input_modalities: Option<Vec<String>>,
    output_modalities: Option<Vec<String>>,
    architecture: Option<Architecture>,
    endpoints: Option<Vec<String>>,
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
    let mut models = BTreeMap::new();
    for model in list.data {
        if model.id.is_empty()
            || model.id.starts_with('$')
            || model
                .id
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
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
        models.insert(
            model.id.clone(),
            DiscoveredModel {
                id: model.id,
                name: model.name,
                capabilities: ModelCapabilityDeclaration {
                    reasoning_efforts: effort.supported_levels.or(reasoning.supported_efforts),
                    reasoning_default: effort.default_level.or(reasoning.default_effort),
                    input_modalities: model.input_modalities.or(architecture.input_modalities),
                    output_modalities: model.output_modalities.or(architecture.output_modalities),
                    interfaces: model.endpoints,
                    protocols: model.api_capabilities.unwrap_or_default(),
                },
            },
        );
    }
    Ok(models.into_values().collect())
}

fn discovery_error(message: &str) -> Error {
    Error::InvalidArgument(format!(
        "Model discovery: {message}. Check the URL and credential or enter the model ID manually."
    ))
}

#[cfg(test)]
mod tests {
    use super::parse_model_list;

    #[test]
    fn provider_model_lists_keep_omissions_and_protocol_scope() {
        let models = parse_model_list(br#"{"data":[
            {"id":"ids-only"},
            {"id":"deepseek-flash","effort":{"supported_levels":["low","high"],"default_level":"high"},
            "input_modalities":["text","image"],"output_modalities":["text"],
            "api_capabilities":{"anthropic_messages":{"system_prompt_update":"in-history"}}},
            {"id":"router/model","reasoning":{"supported_efforts":["none","max"],"default_effort":"max"},
            "architecture":{"input_modalities":["text"],"output_modalities":["text","image"]},
            "endpoints":["/api/v1/responses"]}
        ]}"#).unwrap();
        assert_eq!(None, models[1].capabilities.reasoning_efforts);
        assert_eq!(None, models[1].capabilities.output_modalities);
        assert_eq!(
            Some("high"),
            models[0].capabilities.reasoning_default.as_deref()
        );
        assert_eq!(
            Some("in-history"),
            models[0].capabilities.protocols["anthropic_messages"]
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
    }
}
