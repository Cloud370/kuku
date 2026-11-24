use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result};

/// Authentication scheme used independently of the connection's wire protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderAuth {
    /// Send the credential in the Authorization Bearer header.
    Bearer,
    /// Send the credential in the x-api-key header.
    #[serde(rename = "x-api-key")]
    ApiKey,
}

/// Endpoint family used to interpret model capabilities and native settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderProfile {
    /// Detect a known family from the endpoint hostname.
    #[default]
    Auto,
    /// Use documented native OpenAI settings for recognized models.
    #[serde(rename = "openai")]
    OpenAi,
    /// Use documented native Anthropic settings for recognized models.
    Anthropic,
    /// Use DeepSeek's documented compatibility behavior.
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// Use OpenRouter routing, authentication, and reasoning behavior.
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// Use explicitly declared capabilities for another compatible endpoint.
    Compatible,
}

/// Scope used to group compatible Responses prompts for provider-side caching.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptCacheScope {
    /// Share a cache key across Sessions and Agents in the same workspace.
    #[default]
    Workspace,
    /// Share only within the same Session and Agent address.
    Session,
    /// Do not send a Responses prompt-cache key.
    Off,
}

/// Optional connection behavior and capability declarations.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderOptions {
    /// A renewable subscription account used by this connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<super::SubscriptionAccount>,
    /// Exact model request URL, taking precedence over the base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    /// Optional explicit read-only model-list URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_url: Option<String>,
    /// Network path shared by all calls for this connection.
    #[serde(default, skip_serializing_if = "ProviderProxy::follows_environment")]
    pub proxy: ProviderProxy,
    /// Per-model USD accounting and price preferences.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_pricing: BTreeMap<String, super::ModelPricing>,
    /// Public route price snapshots keyed by exact model ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_price_catalogs: BTreeMap<String, super::PriceCatalog>,
    /// Model declarations owned by this connection, keyed by the exact model identifier.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_capabilities: BTreeMap<String, super::ModelCapabilityEntry>,
    /// Override the endpoint's default authentication scheme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProviderAuth>,
    /// Choose an endpoint family when hostname detection is insufficient.
    #[serde(default)]
    pub profile: ProviderProfile,
    /// Declare supported native reasoning values for a custom endpoint or model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    /// Declare supported native service tiers for a custom endpoint or model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tiers: Option<Vec<String>>,
    /// Apply OpenRouter's provider selection rules without changing the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<OpenRouterRouting>,
    /// Scope for Responses prompt caching; ignored by other protocols.
    #[serde(default)]
    pub prompt_cache_scope: PromptCacheScope,
}

/// How the HTTP transport reaches a provider connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum ProviderProxy {
    #[default]
    Environment,
    Direct,
    Explicit {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<ProxyAuth>,
    },
}

/// References for the password of an authenticated proxy.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyAuth {
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_file: Option<PathBuf>,
}

impl ProviderProxy {
    pub fn follows_environment(&self) -> bool {
        matches!(self, Self::Environment)
    }

    pub fn validate(&self) -> Result<()> {
        let Self::Explicit { url, auth } = self else {
            return Ok(());
        };
        if !Url::parse(url).is_ok_and(|parsed| {
            matches!(parsed.scheme(), "http" | "socks5" | "socks5h")
                && parsed.host_str().is_some()
                && parsed.port_or_known_default().is_some()
                && matches!(parsed.path(), "" | "/")
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.query().is_none()
                && parsed.fragment().is_none()
        }) {
            return Err(Error::ConfigLoad(
                "Use an http://, socks5:// or socks5h:// proxy address with a valid port and no embedded credentials.".into(),
            ));
        }
        if let Some(auth) = auth {
            auth.password_reference()?;
        }
        Ok(())
    }
}

impl ProxyAuth {
    /// Validate and return the reference without reading its password.
    pub fn password_reference(&self) -> Result<super::ApiKey> {
        if !self.username.is_empty() && !self.username.chars().any(char::is_control) {
            match (&self.password_env, &self.password_file) {
                (Some(name), None) if !name.trim().is_empty() => {
                    return Ok(super::ApiKey::Env(name.clone()));
                }
                (None, Some(path)) if path.is_absolute() => {
                    return Ok(super::ApiKey::File(path.clone()));
                }
                _ => {}
            }
        }
        Err(Error::ConfigLoad(
            "Proxy authentication requires a username and one password environment variable or absolute file path.".into(),
        ))
    }
}

/// OpenRouter upstream routing constraints for this connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterRouting {
    /// Prefer these exact upstream endpoint tags in the given order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Restrict eligible upstreams without ordering them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    /// Control upstream fallback without enabling model fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    /// Require upstream support for every requested parameter.
    #[serde(default = "super::types::default_true")]
    pub require_parameters: bool,
    /// Limit eligible input and output unit prices in USD per million tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_price: Option<OpenRouterPrice>,
}

impl Default for OpenRouterRouting {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            only: Vec::new(),
            allow_fallbacks: None,
            require_parameters: true,
            max_price: None,
        }
    }
}

/// OpenRouter unit-price limits, separate from any total run budget.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterPrice {
    /// Maximum input price in USD per million tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<f64>,
    /// Maximum output price in USD per million tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<f64>,
}
