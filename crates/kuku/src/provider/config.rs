use crate::config::{
    Config, ProviderConfig as CfgProvider, ProviderFormat, ThinkLevel, TierConfig,
};
use crate::error::{Error, Result};

use super::capabilities::{resolve_capabilities, resolve_settings};
use super::types::{Provider, ProviderKind, ResolvedProvider, SecretString};

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolveConfigInput {
    pub(crate) connection: Option<String>,
    pub(crate) provider: Option<Provider>,
    pub(crate) model: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) max_output_tokens_per_request: Option<u32>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) config: Option<Config>,
}

pub(crate) fn resolve_config(input: ResolveConfigInput) -> Result<ResolvedProvider> {
    let cfg = input.config.as_ref();
    let tier = select_tier(cfg, input.tier.as_deref())?;
    let (name, connection) = select_connection(cfg, tier, &input)?;
    let kind = match connection {
        Some(connection) => match connection.format {
            ProviderFormat::Anthropic => ProviderKind::Anthropic,
            ProviderFormat::OpenAiChat => ProviderKind::OpenAiCompatible,
            ProviderFormat::OpenAiResponses => ProviderKind::OpenAiResponses,
        },
        None => input.provider.map(ProviderKind::from).ok_or_else(|| {
            Error::MissingProviderConfig(format!("connection '{name}' is not configured"))
        })?,
    };
    if input
        .provider
        .is_some_and(|provider| ProviderKind::from(provider) != kind)
    {
        return Err(Error::MissingProviderConfig(format!(
            "connection '{name}' uses a different wire protocol from the requested provider"
        )));
    }
    let model = input
        .model
        .clone()
        .or_else(|| tier.map(|tier| tier.model.clone()))
        .ok_or_else(|| {
            Error::MissingProviderConfig("no model configured; choose a model or tier".into())
        })?;
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
    let capabilities = resolve_capabilities(&base_url, &model, kind, &options);
    let settings_tier = tier.filter(|tier| input.tier.is_some() || tier.provider == name);
    let think = settings_tier.map(|tier| tier.think);
    let native_reasoning = input
        .reasoning_effort
        .clone()
        .or_else(|| settings_tier.and_then(|tier| tier.reasoning_effort.clone()));
    let requested_service = input
        .service_tier
        .clone()
        .or_else(|| settings_tier.and_then(|tier| tier.service_tier.clone()));
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
    let requires_output_limit = kind == ProviderKind::Anthropic
        && !matches!(
            capabilities.profile,
            crate::config::ProviderProfile::DeepSeek | crate::config::ProviderProfile::OpenRouter
        );
    let output_tier = if requires_output_limit {
        tier
    } else {
        settings_tier
    };
    let max_output_tokens = input
        .max_output_tokens
        .or_else(|| output_tier.and_then(|tier| tier.max_output_tokens))
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
            "connection '{name}': Anthropic Messages requires max_tokens. Set max_output_tokens for model '{model}' on the tier or request, or provide a host output limit."
        )));
    }
    Ok(ResolvedProvider {
        connection_name: name,
        kind,
        model,
        base_url,
        api_key,
        max_context_tokens: tier.map(|tier| tier.context_window).unwrap_or(200_000),
        max_output_tokens,
        think_level: think.unwrap_or(ThinkLevel::Off),
        thinking: Default::default(),
        options,
        settings,
        capabilities,
    })
}

fn select_tier<'a>(cfg: Option<&'a Config>, name: Option<&str>) -> Result<Option<&'a TierConfig>> {
    match name {
        Some(name) => cfg.and_then(|cfg| cfg.tier(name)).map(Some).ok_or_else(|| {
            Error::MissingProviderConfig(format!("tier '{name}' not found in config"))
        }),
        None => Ok(cfg.and_then(|cfg| cfg.tier(cfg.default_tier()))),
    }
}

fn select_connection<'a>(
    cfg: Option<&'a Config>,
    tier: Option<&TierConfig>,
    input: &ResolveConfigInput,
) -> Result<(String, Option<&'a CfgProvider>)> {
    if let Some(name) = &input.connection {
        let connection = cfg.and_then(|cfg| cfg.provider(name)).ok_or_else(|| {
            let message = if cfg.is_some_and(|cfg| cfg.tier(name).is_some()) {
                format!(
                    "connection '{name}' not found in config. '{name}' is a configured model tier. Set tier={name:?} and omit connection to use that tier's provider."
                )
            } else {
                format!("connection '{name}' not found in config")
            };
            Error::MissingProviderConfig(message)
        })?;
        return Ok((name.clone(), Some(connection)));
    }
    if let Some(tier) = tier {
        let connection = cfg.and_then(|cfg| cfg.provider(&tier.provider));
        if input.provider.is_none()
            || connection
                .is_some_and(|connection| protocol_matches(input.provider, connection.format))
        {
            return Ok((tier.provider.clone(), connection));
        }
    }
    if let Some(provider) = input.provider {
        if input.base_url.is_some() && input.api_key.is_some() {
            return Ok((ProviderKind::from(provider).as_str().into(), None));
        }
        return Err(Error::MissingProviderConfig(
            "choose a named connection; a protocol does not identify a provider connection".into(),
        ));
    }
    Err(Error::MissingProviderConfig(
        "no connection configured; choose a named connection or tier".into(),
    ))
}

fn protocol_matches(provider: Option<Provider>, format: ProviderFormat) -> bool {
    matches!(
        (provider, format),
        (Some(Provider::Anthropic), ProviderFormat::Anthropic)
            | (Some(Provider::OpenAiCompatible), ProviderFormat::OpenAiChat)
            | (
                Some(Provider::OpenAiResponses),
                ProviderFormat::OpenAiResponses
            )
    )
}
