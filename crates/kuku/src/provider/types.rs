use std::fmt;
use std::path::PathBuf;

use bytes::Bytes;
use serde_json::Value;

use crate::config::{
    ApiKey, MediaAdapterSupport, ProviderOptions, ProviderProfile, RequestedOutput,
};
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

impl From<crate::config::ProviderFormat> for ProviderKind {
    fn from(format: crate::config::ProviderFormat) -> Self {
        use crate::config::ProviderFormat;
        match format {
            ProviderFormat::Anthropic => Self::Anthropic,
            ProviderFormat::OpenAiChat => Self::OpenAiCompatible,
            ProviderFormat::OpenAiResponses => Self::OpenAiResponses,
        }
    }
}

impl ProviderKind {
    pub(crate) fn format(self) -> crate::config::ProviderFormat {
        use crate::config::ProviderFormat;
        match self {
            Self::Anthropic => ProviderFormat::Anthropic,
            Self::OpenAiCompatible => ProviderFormat::OpenAiChat,
            Self::OpenAiResponses => ProviderFormat::OpenAiResponses,
        }
    }

    pub(crate) fn discovery_protocol(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic_messages",
            Self::OpenAiCompatible => "chat_completions",
            Self::OpenAiResponses => "responses",
        }
    }

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
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) output: RequestedOutput,
    pub(crate) think_level: crate::config::ThinkLevel,
    pub(crate) thinking: crate::config::ResolvedThinking,
    pub(crate) options: ProviderOptions,
    pub(crate) route_request: Option<crate::config::RequestFields>,
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
            max_output_tokens: None,
            output: RequestedOutput::Text,
            think_level: crate::config::ThinkLevel::Off,
            thinking: Default::default(),
            options: Default::default(),
            route_request: None,
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

/// Capabilities declared for the configured endpoint and model.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProviderCapabilities {
    /// Endpoint family that interprets these settings.
    pub profile: ProviderProfile,
    /// Subscription channel used by the configured connection, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_channel: Option<crate::config::SubscriptionChannel>,
    /// Whether the gateway accepts the Anthropic cache-control extension on the selected route.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub prompt_cache_control: bool,
    /// Whether the selected route is known to accept native tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<bool>,
    /// Supported native reasoning values, including none when available.
    pub reasoning_efforts: Vec<String>,
    /// Results from trial requests to this generation endpoint.
    pub thinking_test: Option<crate::config::ModelThinkingTest>,
    /// Supported native service tiers.
    pub service_tiers: Vec<String>,
    /// Additional service tiers supplied by the user and their probe evidence.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub custom_service_tiers: std::collections::BTreeMap<String, crate::config::CustomServiceTier>,
    /// Origin of the service choices, including an explicitly empty declaration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier_source: Option<CapabilitySource>,
    /// Source of the known native reasoning options.
    pub reasoning_source: Option<CapabilitySource>,
    /// Declared default reasoning effort.
    pub reasoning_default: Option<DeclaredCapability<String>>,
    /// Declared model input media.
    pub input_modalities: Option<DeclaredCapability<Vec<String>>>,
    /// Declared model output media.
    pub output_modalities: Option<DeclaredCapability<Vec<String>>>,
    /// Provider-advertised interfaces.
    pub interfaces: Option<DeclaredCapability<Vec<String>>>,
    /// Declared system prompt behavior on the selected protocol.
    pub system_prompt_update: Option<DeclaredCapability<String>>,
    /// Media representation implemented by the runtime.
    pub media_adapter: MediaAdapterSupport,
}

impl ProviderCapabilities {
    /// Values to offer in a selector. Explicit manual options remain user-controlled.
    pub fn selectable_reasoning_efforts(&self) -> impl Iterator<Item = &str> {
        let test = self.thinking_test.as_ref().filter(|_| {
            !matches!(
                self.reasoning_source,
                Some(CapabilitySource::Manual | CapabilitySource::ProtocolManual)
            )
        });
        let options = test.map_or(self.reasoning_efforts.as_slice(), |test| {
            test.accepted.as_slice()
        });
        options.iter().map(String::as_str).filter(move |value| {
            test.is_none_or(|test| !test.rejected.iter().any(|level| level == value))
        })
    }
}

/// Origin and scope of a normalized model capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySource {
    /// Declared by the provider model list.
    ModelList,
    /// Declared for the selected protocol by the provider model list.
    ProtocolModelList,
    /// Entered manually for the connection or model.
    Manual,
    /// Entered manually for the selected protocol.
    ProtocolManual,
    /// Defined by a built-in provider profile.
    AdapterProfile,
}

/// A capability with the provenance of its effective value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeclaredCapability<T> {
    /// Resolved value.
    pub value: T,
    /// Origin and scope.
    pub source: CapabilitySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderCorrelation {
    pub(crate) session_id: String,
    pub(crate) agent_address: String,
    pub(crate) connection_name: String,
    pub(crate) workspace: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PreparedProviderRequest {
    pub(crate) kind: ProviderKind,
    pub(crate) connection_name: String,
    pub(crate) model: String,
    pub(crate) url: String,
    pub(crate) body: PreparedBody,
    pub(crate) settings: ProviderSettings,
    pub(crate) correlation_id: Option<String>,
    pub(super) fields: PreparedRequestFields,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct PreparedRequestFields {
    pub(super) headers: wreq::header::HeaderMap,
    pub(super) request_id_headers: Vec<wreq::header::HeaderName>,
    pub(super) private_body_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PreparedBody {
    json: Value,
    bytes: Bytes,
}

impl PreparedBody {
    pub(crate) fn from_json(json: Value) -> Self {
        let bytes = serde_json::to_vec(&json)
            .expect("provider request body is JSON")
            .into();
        Self { json, bytes }
    }

    pub(crate) fn as_json(&self) -> &Value {
        &self.json
    }

    pub(crate) fn as_bytes(&self) -> &Bytes {
        &self.bytes
    }
}

/// Optional settings and usage reported by the serving provider.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderReport {
    /// Model name reported by the response.
    pub model: Option<String>,
    /// Upstream provider name, when the gateway reports one.
    pub provider: Option<String>,
    /// Whether the gateway used a customer-supplied upstream credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_byok: Option<bool>,
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
    PermissionDenied,
    QuotaExhausted,
    RateLimited,
    Timeout,
    ContextTooLarge,
    InvalidRequest,
    InvalidResponse,
    ProviderUnavailable,
    Transport,
    Internal,
    Unknown,
}

impl ProviderFailureKind {
    pub(crate) fn as_event_kind(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::PermissionDenied => "permission_denied",
            Self::QuotaExhausted => "quota_exhausted",
            Self::ContextTooLarge => "context_too_large",
            Self::InvalidRequest => "invalid_request",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::Transport => "transport",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
            Self::InvalidResponse => "invalid_response",
        }
    }

    pub(crate) fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Timeout | Self::ProviderUnavailable | Self::Transport
        )
    }

    pub(crate) fn safe_retry_message(self) -> &'static str {
        match self {
            Self::Authentication => "provider authentication failed; check configured credentials",
            Self::PermissionDenied => "provider access denied; check provider permissions",
            Self::QuotaExhausted => "provider quota exhausted; check billing or quota",
            Self::RateLimited => "provider rate limited",
            Self::Timeout => "provider request timed out",
            Self::ContextTooLarge => "provider context limit exceeded; reduce request context",
            Self::InvalidRequest => "provider rejected the request; check request parameters",
            Self::InvalidResponse => "provider returned an invalid response",
            Self::ProviderUnavailable => "provider unavailable",
            Self::Transport => "provider transport failed",
            Self::Internal => "internal provider error",
            Self::Unknown => "unknown provider error",
        }
    }
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
