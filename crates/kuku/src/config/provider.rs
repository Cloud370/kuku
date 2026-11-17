use serde::{Deserialize, Serialize};

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

/// Optional connection behavior and capability declarations.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderOptions {
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
