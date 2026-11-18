use std::fmt;

use crate::config::{ApiKey, ProviderOptions, ProviderProfile};
use crate::context::CanonicalMessage;
use crate::context::ContextAssembly;
use crate::prompt::PromptCatalog;

/// Public provider selector for the query builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAiCompatible,
    OpenAiResponses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ProviderKind {
    Anthropic,
    OpenAiCompatible,
    OpenAiResponses,
}

impl From<Provider> for ProviderKind {
    fn from(provider: Provider) -> Self {
        match provider {
            Provider::Anthropic => ProviderKind::Anthropic,
            Provider::OpenAiCompatible => ProviderKind::OpenAiCompatible,
            Provider::OpenAiResponses => ProviderKind::OpenAiResponses,
        }
    }
}

impl ProviderKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::OpenAiResponses => "openai-responses",
        }
    }
}

/// Wrapper that redacts the value in Debug/Display.
#[derive(Clone, PartialEq)]
pub(crate) struct SecretString(ApiKey);

impl SecretString {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(ApiKey::Plaintext(value.into()))
    }

    pub(crate) fn from_config(value: ApiKey) -> Self {
        Self(value)
    }

    pub(crate) fn resolve(&self) -> crate::error::Result<String> {
        self.0.resolve()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedProvider {
    pub(crate) connection_name: String,
    pub(crate) kind: ProviderKind,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) api_key: SecretString,
    pub(crate) max_context_tokens: u32,
    pub(crate) max_output_tokens: u32,
    pub(crate) think_level: crate::config::ThinkLevel,
    pub(crate) thinking: crate::config::ResolvedThinking,
    pub(crate) options: ProviderOptions,
    pub(crate) settings: ProviderSettings,
    pub(crate) capabilities: ProviderCapabilities,
}

impl Default for ResolvedProvider {
    fn default() -> Self {
        Self {
            connection_name: "default".into(),
            kind: ProviderKind::OpenAiCompatible,
            model: "test-model".into(),
            base_url: "http://localhost".into(),
            api_key: SecretString::new(""),
            max_context_tokens: 200_000,
            max_output_tokens: 4096,
            think_level: crate::config::ThinkLevel::Off,
            thinking: Default::default(),
            options: Default::default(),
            settings: Default::default(),
            capabilities: Default::default(),
        }
    }
}

/// Requested and effective settings for one resolved model connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderSettings {
    /// Native reasoning or portable thinking requested by the caller or tier.
    pub requested_reasoning_effort: Option<String>,
    /// Effective native reasoning value, absent when the wire request omits it.
    pub reasoning_effort: Option<String>,
    /// Service tier requested independently of reasoning.
    pub requested_service_tier: Option<String>,
    /// Service tier actually encoded into the request.
    pub service_tier: Option<String>,
}

/// Native settings declared for the configured endpoint and model.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCapabilities {
    /// Endpoint family that interprets these settings.
    pub profile: ProviderProfile,
    /// Supported native reasoning values, including none when available.
    pub reasoning_efforts: Vec<String>,
    /// Supported native service tiers.
    pub service_tiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderCorrelation {
    pub(crate) session_id: String,
    pub(crate) agent_address: String,
    pub(crate) connection_name: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PreparedProviderRequest {
    pub(crate) kind: ProviderKind,
    pub(crate) connection_name: String,
    pub(crate) model: String,
    pub(crate) url: String,
    pub(crate) body: serde_json::Value,
    pub(crate) settings: ProviderSettings,
    pub(crate) correlation_id: Option<String>,
}

/// Optional settings and usage reported by the serving provider.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderReport {
    /// Model name reported by the response.
    pub model: Option<String>,
    /// Upstream provider name, when the gateway reports one.
    pub provider: Option<String>,
    /// Native service tier reported by the response.
    pub service_tier: Option<String>,
    /// Native reasoning setting reported by the response.
    pub reasoning_effort: Option<String>,
    /// Final request cost in USD, when reported.
    pub cost_usd: Option<f64>,
    /// Unmodified available token and billing counters.
    pub raw_usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderRequest<'a> {
    pub(crate) assembly: ContextAssembly,
    pub(crate) catalog: &'a PromptCatalog,
    pub(crate) current_input: CanonicalPromptInput,
    pub(crate) model: String,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) temperature: Option<f32>,
    pub(crate) stream: bool,
    pub(crate) think_level: crate::config::ThinkLevel,
    pub(crate) thinking: crate::config::ResolvedThinking,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CanonicalPromptInput {
    pub(crate) parts: Vec<CanonicalMessage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderUsage {
    /// Total input tokens, including cached input when reported.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

impl ProviderUsage {
    pub(crate) fn cache_input_counts(&self) -> Option<(u64, u64)> {
        let input = self.input_tokens?;
        let read = self.cache_read_input_tokens?;
        let written = self.cache_creation_input_tokens.unwrap_or(0);
        (read.checked_add(written)? <= input).then_some((read, input))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: serde_json::Value,
    pub(crate) index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailureKind {
    Authentication,
    RateLimited,
    ContextTooLarge,
    InvalidRequest,
    ProviderUnavailable,
    Transport,
    Internal,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderFailure {
    pub(crate) kind: ProviderFailureKind,
    pub(crate) message: String,
    pub(crate) status: Option<u16>,
    pub(crate) provider_request_id: Option<String>,
    pub(crate) retry_after: Option<std::time::Duration>,
    pub(crate) retryable: bool,
}
