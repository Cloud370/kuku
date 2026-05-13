use crate::error::{Error, Result};

use super::types::{Provider, ProviderKind, ResolvedProvider, SecretString};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolveConfigInput {
    pub(crate) provider: Option<Provider>,
    pub(crate) model: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: Option<String>,
}

pub(crate) const ENV_PROVIDER: &str = "KUKU_PROVIDER";
pub(crate) const ENV_MODEL: &str = "KUKU_MODEL";
pub(crate) const ENV_BASE_URL: &str = "KUKU_BASE_URL";
pub(crate) const ENV_API_KEY: &str = "KUKU_API_KEY";

pub(crate) const ENV_ANTHROPIC_MODEL: &str = "KUKU_ANTHROPIC_MODEL";
pub(crate) const ENV_ANTHROPIC_BASE_URL: &str = "KUKU_ANTHROPIC_BASE_URL";
pub(crate) const ENV_ANTHROPIC_API_KEY: &str = "KUKU_ANTHROPIC_API_KEY";

pub(crate) const ENV_OPENAI_MODEL: &str = "KUKU_OPENAI_MODEL";
pub(crate) const ENV_OPENAI_BASE_URL: &str = "KUKU_OPENAI_BASE_URL";
pub(crate) const ENV_OPENAI_API_KEY: &str = "KUKU_OPENAI_API_KEY";

pub(crate) fn resolve_config(input: ResolveConfigInput) -> Result<ResolvedProvider> {
    let provider = match input.provider {
        Some(provider) => ProviderKind::from(provider),
        None => match env_opt(ENV_PROVIDER) {
            Some(value) => parse_provider(&value)?,
            None => infer_provider_from_provider_specific_env()?,
        },
    };

    let (provider_model, provider_base_url, provider_api_key) = provider_specific_values(&provider);

    let model = input
        .model
        .or(provider_model)
        .or_else(|| env_opt(ENV_MODEL))
        .ok_or_else(|| {
            Error::MissingProviderConfig(
                "no model configured; set builder .model(...) or env".to_string(),
            )
        })?;

    let base_url = input
        .base_url
        .or(provider_base_url)
        .or_else(|| env_opt(ENV_BASE_URL))
        .unwrap_or_else(|| default_base_url(&provider).to_string());

    let api_key = input
        .api_key
        .or(provider_api_key)
        .or_else(|| env_opt(ENV_API_KEY))
        .ok_or_else(|| {
            Error::MissingProviderConfig(
                "no API key configured; set builder .api_key(...) or env".to_string(),
            )
        })?;

    Ok(ResolvedProvider {
        kind: provider,
        model,
        base_url,
        api_key: SecretString::new(api_key),
    })
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn parse_provider(value: &str) -> Result<ProviderKind> {
    match value {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "openai-compatible" | "openai" => Ok(ProviderKind::OpenAiCompatible),
        other => Err(Error::MissingProviderConfig(format!(
            "unknown KUKU_PROVIDER value: {other}"
        ))),
    }
}

fn infer_provider_from_provider_specific_env() -> Result<ProviderKind> {
    let has_anthropic = env_opt(ENV_ANTHROPIC_API_KEY).is_some();
    let has_openai = env_opt(ENV_OPENAI_API_KEY).is_some();

    match (has_anthropic, has_openai) {
        (true, false) => Ok(ProviderKind::Anthropic),
        (false, true) => Ok(ProviderKind::OpenAiCompatible),
        (true, true) => Err(Error::AmbiguousProviderConfig(
            "both KUKU_ANTHROPIC_API_KEY and KUKU_OPENAI_API_KEY are set; set KUKU_PROVIDER to disambiguate"
                .to_string(),
        )),
        (false, false) => Err(Error::MissingProviderConfig(
            "no provider configured; set KUKU_PROVIDER or provider-specific env vars".to_string(),
        )),
    }
}

fn provider_specific_values(
    kind: &ProviderKind,
) -> (Option<String>, Option<String>, Option<String>) {
    match kind {
        ProviderKind::Anthropic => (
            env_opt(ENV_ANTHROPIC_MODEL),
            env_opt(ENV_ANTHROPIC_BASE_URL),
            env_opt(ENV_ANTHROPIC_API_KEY),
        ),
        ProviderKind::OpenAiCompatible => (
            env_opt(ENV_OPENAI_MODEL),
            env_opt(ENV_OPENAI_BASE_URL),
            env_opt(ENV_OPENAI_API_KEY),
        ),
    }
}

fn default_base_url(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com",
        ProviderKind::OpenAiCompatible => "https://api.openai.com/v1",
    }
}
