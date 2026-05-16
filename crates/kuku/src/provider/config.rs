use crate::config::{ApiKeySource, Config};
use crate::error::{Error, Result};

use super::types::{Provider, ProviderKind, ResolvedProvider, SecretString};

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolveConfigInput {
    pub(crate) provider: Option<Provider>,
    pub(crate) model: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) config: Option<Config>,
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
    // 1. Resolve provider kind (explicit → env → config → provider-specific env inference)
    let kind = match input.provider {
        Some(provider) => ProviderKind::from(provider),
        None => match env_opt(ENV_PROVIDER) {
            Some(value) => parse_provider(&value)?,
            None => match input.config.as_ref().and_then(resolve_provider_from_config) {
                Some(kind) => kind,
                None => infer_provider_from_provider_specific_env()?,
            },
        },
    };

    // 2. Provider alias for config lookup (built-in or custom)
    let provider_alias = match &input.config {
        Some(cfg) => provider_alias_for_kind(&kind, cfg),
        None => kind.as_str().to_string(),
    };

    // 3. Provider-specific env values (backwards compat)
    let (ps_model, ps_base_url, ps_api_key) = provider_specific_values(&kind);

    // 4. Model: explicit → provider-specific env → generic env → config → default
    let model = if let Some(model) = input.model {
        resolve_model_alias(&model, input.config.as_ref())
    } else if let Some(model) = ps_model {
        model
    } else if let Some(model) = env_opt(ENV_MODEL) {
        model
    } else if let Some(ref cfg) = input.config {
        let default_alias = cfg.default_model();
        cfg.resolve_model_alias(default_alias)
            .map(|target| target.split(':').nth(1).unwrap_or(target).to_string())
            .unwrap_or_else(|| provider_default_model(&kind).to_string())
    } else {
        return Err(Error::MissingProviderConfig(
            "no model configured; set builder .model(...) or env".to_string(),
        ));
    };

    // 5. Base URL: explicit → provider-specific env → generic env → config → default
    let base_url = input
        .base_url
        .or(ps_base_url)
        .or_else(|| env_opt(ENV_BASE_URL))
        .or_else(|| {
            input.config.as_ref().and_then(|cfg| {
                cfg.provider(&provider_alias)
                    .and_then(|p| p.base_url.clone())
            })
        })
        .unwrap_or_else(|| default_base_url(&kind).to_string());

    // 6. API key: explicit → provider-specific env → generic env → config → error
    let api_key = input
        .api_key
        .or(ps_api_key)
        .or_else(|| env_opt(ENV_API_KEY))
        .or_else(|| {
            input.config.as_ref().and_then(|cfg| {
                cfg.provider(&provider_alias)
                    .and_then(|p| match &p.api_key {
                        ApiKeySource::Plaintext(key) => Some(key.clone()),
                        ApiKeySource::Env(env_name) => env_opt(env_name),
                    })
            })
        })
        .ok_or_else(|| {
            Error::MissingProviderConfig(
                "no API key configured; set api_key in config file or env".to_string(),
            )
        })?;

    // 7. Thinking config: config file only (uses provider alias, not resolved kind)
    let thinking = input
        .config
        .as_ref()
        .and_then(|cfg| cfg.provider(&provider_alias))
        .map(|p| p.thinking.clone())
        .unwrap_or_default();

    Ok(ResolvedProvider {
        kind,
        model,
        base_url,
        api_key: SecretString::new(api_key),
        max_context_tokens: default_max_context_tokens(),
        thinking,
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

fn resolve_provider_from_config(cfg: &Config) -> Option<ProviderKind> {
    let default_model = cfg.default_model();
    let target = cfg.resolve_model_alias(default_model)?;
    let provider_name = target.split(':').next().unwrap_or("anthropic");
    match provider_name {
        "anthropic" => Some(ProviderKind::Anthropic),
        "openai" | "openai-compatible" => Some(ProviderKind::OpenAiCompatible),
        other => cfg.provider(other).and_then(|p| match p.format.as_str() {
            "anthropic" => Some(ProviderKind::Anthropic),
            "openai" | "openai-compatible" => Some(ProviderKind::OpenAiCompatible),
            _ => None,
        }),
    }
}

fn provider_alias_for_kind(kind: &ProviderKind, cfg: &Config) -> String {
    let built_in = kind.as_str();
    if cfg.provider(built_in).is_some() {
        return built_in.to_string();
    }
    for (alias, provider_cfg) in &cfg.providers {
        let found = matches!(
            (kind, provider_cfg.format.as_str()),
            (ProviderKind::Anthropic, "anthropic")
                | (
                    ProviderKind::OpenAiCompatible,
                    "openai" | "openai-compatible"
                )
        );
        if found {
            return alias.clone();
        }
    }
    built_in.to_string()
}

fn resolve_model_alias(model: &str, cfg: Option<&Config>) -> String {
    if let Some(cfg) = cfg {
        if let Some(target) = cfg.resolve_model_alias(model) {
            return target.split(':').nth(1).unwrap_or(target).to_string();
        }
    }
    model.to_string()
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

fn provider_default_model(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "claude-sonnet-4-6",
        ProviderKind::OpenAiCompatible => "gpt-5-mini",
    }
}

fn default_base_url(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com",
        ProviderKind::OpenAiCompatible => "https://api.openai.com/v1",
    }
}

fn default_max_context_tokens() -> u32 {
    200_000
}
