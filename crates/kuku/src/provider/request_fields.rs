use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;
use sha2::{Digest, Sha256};
use wreq::header::{HeaderName, HeaderValue};

use crate::config::{ModelPricing, ProviderProfile, RequestFields, SubscriptionChannel};

use super::prepare::{invalid_request, is_official_openrouter};
use super::types::{
    PreparedRequestFields, ProviderCorrelation, ProviderFailure, ProviderKind, ResolvedProvider,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestPurpose {
    Inference,
    SingleRequest,
}

pub(super) fn correlation_id(correlation: &ProviderCorrelation) -> String {
    let mut hash = Sha256::new();
    for value in [
        &correlation.session_id,
        &correlation.agent_address,
        &correlation.connection_name,
    ] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    format!("kuku-{:x}", hash.finalize())
}

pub(super) fn prepare(
    config: &ResolvedProvider,
    url: &str,
    body: &mut Value,
    purpose: RequestPurpose,
    session_id: Option<&str>,
    make_cache_key: impl Fn() -> Option<String>,
) -> Result<PreparedRequestFields, ProviderFailure> {
    let mut fields = defaults(config, url, purpose)?;
    let mut private_body_keys = Vec::new();
    for custom in [&config.options.request, &config.route_request]
        .into_iter()
        .flatten()
    {
        custom
            .validate()
            .map_err(|error| invalid_request(&error.to_string()))?;
        if !custom.inherit {
            private_body_keys.clear();
        }
        for name in custom.body.keys() {
            if !private_body_keys.contains(name) {
                private_body_keys.push(name.clone());
            }
        }
        overlay(&mut fields, custom);
    }
    if chatgpt_subscription(config) {
        if let Some(field) = fields.body.keys().find(|field| {
            matches!(
                field.as_str(),
                "background"
                    | "conversation"
                    | "max_tool_calls"
                    | "metadata"
                    | "moderation"
                    | "multi_agent"
                    | "prompt"
                    | "prompt_cache_retention"
                    | "safety_identifier"
                    | "temperature"
                    | "top_logprobs"
                    | "top_p"
                    | "truncation"
                    | "user"
            )
        }) {
            return Err(invalid_request(&format!(
                "request body field '{field}' is unavailable on ChatGPT sign-in"
            )));
        }
    }
    let key = OnceCell::new();
    let lookup = |value: &str| match value {
        "{{session_id}}" => session_id.map(str::to_owned),
        "{{prompt_cache_key}}" => key.get_or_init(&make_cache_key).clone(),
        _ => Some(value.to_owned()),
    };

    let mut prepared = PreparedRequestFields {
        private_body_keys,
        ..Default::default()
    };
    for (name, value) in fields.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| invalid_request("invalid configured HTTP header name"))?;
        if value == "{{request_id}}" {
            prepared.request_id_headers.push(name);
            continue;
        }
        let value = if let Some(var) = value.strip_prefix('$') {
            Some(std::env::var(var).map_err(|_| {
                invalid_request(&format!(
                    "environment variable for request header '{name}' is unavailable"
                ))
            })?)
        } else {
            lookup(&value)
        };
        if let Some(value) = value {
            let mut header = HeaderValue::from_str(&value).map_err(|_| {
                invalid_request(&format!("invalid value for request header '{name}'"))
            })?;
            header.set_sensitive(true);
            prepared.headers.insert(name, header);
        }
    }
    let object = body.as_object_mut().expect("provider request is an object");
    for (name, value) in fields.body {
        if let Some(value) = resolve_body(value, &lookup) {
            object.insert(name, value);
        }
    }
    Ok(prepared)
}

fn chatgpt_subscription(config: &ResolvedProvider) -> bool {
    config
        .options
        .subscription
        .as_ref()
        .is_some_and(|account| account.channel() == SubscriptionChannel::ChatGpt)
}

fn defaults(
    config: &ResolvedProvider,
    url: &str,
    purpose: RequestPurpose,
) -> Result<RequestFields, ProviderFailure> {
    static PRESETS: OnceLock<BTreeMap<String, RequestFields>> = OnceLock::new();
    let presets = PRESETS.get_or_init(|| {
        toml::from_str(include_str!("../../assets/request-presets.toml"))
            .expect("valid bundled request presets")
    });
    let mut fields = RequestFields::default();
    let profile = config.capabilities.profile;
    if chatgpt_subscription(config) {
        overlay(&mut fields, &presets["chatgpt"]);
        return Ok(fields);
    }
    if profile == ProviderProfile::OpenRouter {
        overlay(&mut fields, &presets["openrouter"]);
        if let Some(routing) = &config.options.routing {
            let routing = serde_json::to_value(routing)
                .map_err(|_| invalid_request("invalid OpenRouter routing options"))?;
            fields.body.insert("provider".into(), routing);
        }
        if url::Url::parse(url).is_ok_and(|url| is_official_openrouter(&url))
            && !matches!(
                config.options.model_pricing.get(&config.model),
                Some(ModelPricing::UsageOnly)
            )
        {
            overlay(&mut fields, &presets["openrouter_reports"]);
        }
    }
    if purpose == RequestPurpose::SingleRequest {
        return Ok(fields);
    }
    if config.capabilities.prompt_cache_control
        || (profile == ProviderProfile::Anthropic && config.kind == ProviderKind::Anthropic)
    {
        overlay(&mut fields, &presets["anthropic"]);
    }
    if profile == ProviderProfile::OpenAi
        && config.kind == ProviderKind::OpenAiResponses
        && !has_automatic_cache_routing(&config.model)
    {
        overlay(&mut fields, &presets["openai_legacy"]);
    }
    Ok(fields)
}

fn has_automatic_cache_routing(model: &str) -> bool {
    let version = model
        .strip_prefix("gpt-")
        .and_then(|model| model.split('-').next())
        .unwrap_or_default();
    let (major, minor) = version.split_once('.').unwrap_or((version, "0"));
    matches!(
        (major.parse::<u16>(), minor.parse::<u16>()),
        (Ok(5), Ok(6..)) | (Ok(6..), Ok(_))
    )
}

fn overlay(fields: &mut RequestFields, layer: &RequestFields) {
    if !layer.inherit {
        fields.headers.clear();
        fields.body.clear();
    }
    fields.headers.extend(
        layer
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone())),
    );
    fields.body.extend(layer.body.clone());
}

fn resolve_body(value: Value, lookup: &impl Fn(&str) -> Option<String>) -> Option<Value> {
    match value {
        Value::String(value) => lookup(&value).map(Value::String),
        Value::Array(values) => Some(Value::Array(
            values
                .into_iter()
                .filter_map(|value| resolve_body(value, lookup))
                .collect(),
        )),
        Value::Object(values) => Some(Value::Object(
            values
                .into_iter()
                .filter_map(|(key, value)| Some((key, resolve_body(value, lookup)?)))
                .collect(),
        )),
        other => Some(other),
    }
}
