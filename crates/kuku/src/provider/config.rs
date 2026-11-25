use crate::config::{Config, ModelRef, ThinkLevel};
use crate::error::{Error, Result};

use super::capabilities::{resolve_capabilities, resolve_settings};
use super::types::{Provider, ProviderKind, ResolvedProvider, SecretString};

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolveConfigInput<'a> {
    pub(crate) connection: Option<String>,
    pub(crate) provider: Option<Provider>,
    pub(crate) model: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) max_output_tokens_per_request: Option<u32>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) config: Option<&'a Config>,
    pub(crate) output: Option<crate::config::RequestedOutput>,
}

pub(crate) fn resolve_config(input: ResolveConfigInput<'_>) -> Result<ResolvedProvider> {
    resolve(input, true)
}

pub(crate) fn validate_model_config(config: &Config, model: &ModelRef) -> Result<()> {
    resolve(
        ResolveConfigInput {
            connection: Some(model.connection.clone()),
            model: Some(model.model.clone()),
            config: Some(config),
            ..Default::default()
        },
        model == config.default_model(),
    )?;
    Ok(())
}

fn resolve(input: ResolveConfigInput<'_>, require_output_limit: bool) -> Result<ResolvedProvider> {
    let (name, model, connection, model_config) = if input.connection.is_none()
        && input.base_url.is_some()
        && input.api_key.is_some()
        && let (Some(provider), Some(model)) = (input.provider, &input.model)
    {
        // The standalone Query builder can supply a complete connection directly.
        (
            ProviderKind::from(provider).as_str().into(),
            model.clone(),
            None,
            None,
        )
    } else {
        let config = input.config.ok_or_else(|| {
            Error::MissingProviderConfig("configure a named connection and model".into())
        })?;
        let selection =
            match (&input.connection, &input.model) {
                (Some(connection), Some(model)) => ModelRef::new(connection, model),
                (None, None) => config.default_model.clone(),
                _ => return Err(Error::InvalidArgument(
                    "specify both connection and exact model ID, or omit both to use the default"
                        .into(),
                )),
            };
        let connection = config.provider(&selection.connection).ok_or_else(|| {
            Error::MissingProviderConfig(format!(
                "connection {:?} for model {:?} is not configured",
                selection.connection, selection.model,
            ))
        })?;
        let model_config = config.model(&selection).ok_or_else(|| {
            Error::MissingProviderConfig(format!(
                "model {:?} is not registered on connection {:?}; register [{}] or choose a configured pair",
                selection.model, selection.connection, selection.config_path(),
            ))
        })?;
        (
            selection.connection,
            selection.model,
            Some(connection),
            Some(model_config),
        )
    };
    if model.is_empty() || model.trim() != model {
        return Err(Error::InvalidArgument(
            "model ID must be nonempty with no surrounding whitespace".into(),
        ));
    }
    let kind = match connection {
        Some(connection) => ProviderKind::from(connection.format),
        None => ProviderKind::from(input.provider.expect("complete standalone Query provider")),
    };
    if input
        .provider
        .is_some_and(|provider| ProviderKind::from(provider) != kind)
    {
        return Err(Error::MissingProviderConfig(format!(
            "connection '{name}' uses a different wire protocol from the requested provider"
        )));
    }
    let base_url = input
        .base_url
        .clone()
        .or_else(|| connection.map(|connection| connection.base_url.clone()))
        .ok_or_else(|| {
            Error::MissingProviderConfig(format!("connection '{name}' has no base_url"))
        })?;
    let api_key = match input.api_key.as_ref() {
        Some(key) => SecretString::new(key),
        None => SecretString::from_config(
            connection
                .ok_or_else(|| {
                    Error::MissingProviderConfig(format!("connection '{name}' has no api_key"))
                })?
                .api_key
                .clone(),
        ),
    };
    let options = connection
        .map(|connection| connection.options.clone())
        .unwrap_or_default();
    let endpoint = crate::config::resolve_request_url(
        kind.format(),
        &base_url,
        options.endpoint_url.as_deref(),
    )?;
    let capabilities = resolve_capabilities(&endpoint, &model, kind, &options);
    let route_request = model_config.and_then(|config| config.request.clone());
    let think = model_config.map(|config| config.think);
    let native_reasoning = input
        .reasoning_effort
        .clone()
        .or_else(|| model_config.and_then(|config| config.reasoning_effort.clone()));
    let requested_service = input
        .service_tier
        .clone()
        .or_else(|| model_config.and_then(|config| config.service_tier.clone()));
    let output = input
        .output
        .or_else(|| model_config.map(|config| config.output))
        .unwrap_or_default();
    if output == crate::config::RequestedOutput::Image
        && capabilities
            .output_modalities
            .as_ref()
            .is_some_and(|media| !media.value.iter().any(|item| item == "image"))
    {
        return Err(Error::InvalidArgument(format!(
            "connection '{name}', model '{model}' does not declare image output"
        )));
    }
    let settings = resolve_settings(
        &name,
        &capabilities,
        native_reasoning,
        think,
        requested_service,
    )?;
    if options.routing.is_some()
        && capabilities.profile != crate::config::ProviderProfile::OpenRouter
    {
        return Err(Error::MissingProviderConfig(format!(
            "connection '{name}': routing options require an OpenRouter profile"
        )));
    }
    let requires_output_limit = require_output_limit
        && kind == ProviderKind::Anthropic
        && !matches!(
            capabilities.profile,
            crate::config::ProviderProfile::DeepSeek | crate::config::ProviderProfile::OpenRouter
        );
    let max_output_tokens = input
        .max_output_tokens
        .or_else(|| model_config.and_then(|config| config.max_output_tokens))
        .into_iter()
        .chain(input.max_output_tokens_per_request)
        .min();
    if max_output_tokens == Some(0) {
        return Err(Error::InvalidArgument(
            "max_output_tokens must be positive".into(),
        ));
    }
    if max_output_tokens.is_none() && requires_output_limit {
        return Err(Error::MissingProviderConfig(format!(
            "connection '{name}': Anthropic Messages requires max_tokens. Set max_output_tokens for model '{model}' or on the request, or provide a host output limit."
        )));
    }
    let resolved = ResolvedProvider {
        connection_name: name,
        kind,
        model,
        base_url,
        api_key,
        max_context_tokens: model_config
            .map(|config| config.context_window)
            .unwrap_or(200_000),
        max_output_tokens,
        output,
        think_level: think.unwrap_or(ThinkLevel::Off),
        thinking: Default::default(),
        options,
        route_request,
        settings,
        capabilities,
    };
    resolved
        .validate_output_limit(max_output_tokens)
        .map_err(|message| Error::InvalidArgument(message.into()))?;
    Ok(resolved)
}
