use crate::config::{Config, ProviderConfig as CfgProvider, ThinkLevel, TierConfig};
use crate::error::{Error, Result};

use super::types::{Provider, ProviderKind, ResolvedProvider, SecretString};

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolveConfigInput {
    pub(crate) provider: Option<Provider>,
    pub(crate) model: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) config: Option<Config>,
}

/// Resolve a complete provider configuration from builder inputs + config file.
pub(crate) fn resolve_config(input: ResolveConfigInput) -> Result<ResolvedProvider> {
    let cfg = input.config.as_ref();

    let tier_config: Option<&TierConfig> = if input.model.is_some() {
        cfg.and_then(|c| c.tier(c.default_tier()))
    } else if let Some(ref tier_name) = input.tier {
        Some(cfg.and_then(|c| c.tier(tier_name)).ok_or_else(|| {
            Error::MissingProviderConfig(format!("tier '{tier_name}' not found in config"))
        })?)
    } else {
        cfg.and_then(|c| c.tier(c.default_tier()))
    };

    let provider_name_from_tier = tier_config.map(|tc| tc.provider.as_str());
    let provider_name: &str = match input.provider {
        Some(p) => ProviderKind::from(p).as_str(),
        None => provider_name_from_tier.ok_or_else(|| {
            Error::MissingProviderConfig(
                "no provider configured; set builder .provider() or configure tiers".to_string(),
            )
        })?,
    };

    let provider_cfg: Option<&CfgProvider> = match input.provider {
        Some(p) => {
            let target_format = provider_kind_to_format(ProviderKind::from(p));
            cfg.and_then(|c| c.providers.values().find(|pc| pc.format == target_format))
        }
        None => cfg.and_then(|c| c.provider(provider_name)),
    };

    let model: String = if let Some(ref model) = input.model {
        model.clone()
    } else {
        tier_config.map(|tc| tc.model.clone()).ok_or_else(|| {
            Error::MissingProviderConfig(
                "no model configured; set builder .model(), .tier(), or configure tiers"
                    .to_string(),
            )
        })?
    };

    let think_level = tier_config.map(|tc| tc.think).unwrap_or(ThinkLevel::Medium);

    let api_key: String = if let Some(ref key) = input.api_key {
        key.clone()
    } else if let Some(pc) = provider_cfg {
        pc.api_key.resolve()?
    } else {
        return Err(Error::MissingProviderConfig(format!(
            "no api_key for provider '{provider_name}'; set builder .api_key() or configure [provider.{provider_name}]"
        )));
    };

    let base_url: String = if let Some(ref url) = input.base_url {
        url.clone()
    } else if let Some(pc) = provider_cfg {
        pc.base_url.clone()
    } else {
        return Err(Error::MissingProviderConfig(format!(
            "no base_url for provider '{provider_name}'; set builder .base_url() or configure [provider.{provider_name}]"
        )));
    };

    let context_window = tier_config.map(|tc| tc.context_window).unwrap_or(200_000);

    let max_output_tokens = input
        .max_output_tokens
        .or_else(|| tier_config.map(|tc| tc.max_output_tokens))
        .unwrap_or(48_000);

    let kind: ProviderKind = match provider_cfg.map(|pc| pc.format.as_str()) {
        Some("anthropic") => ProviderKind::Anthropic,
        Some("openai-chat") => ProviderKind::OpenAiCompatible,
        Some("openai-responses") => ProviderKind::OpenAiResponses,
        Some(other) => {
            return Err(Error::MissingProviderConfig(format!(
                "unknown format '{other}' for provider '{provider_name}'"
            )));
        }
        None => {
            return Err(Error::MissingProviderConfig(format!(
                "no provider config for '{provider_name}'; define [provider.{provider_name}] or set builder .provider()"
            )));
        }
    };

    Ok(ResolvedProvider {
        kind,
        model,
        base_url,
        api_key: SecretString::new(api_key),
        max_context_tokens: context_window,
        max_output_tokens,
        think_level,
        thinking: Default::default(),
    })
}

fn provider_kind_to_format(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAiCompatible => "openai-chat",
        ProviderKind::OpenAiResponses => "openai-responses",
    }
}
