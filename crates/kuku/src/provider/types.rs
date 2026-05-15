use std::fmt;

use crate::context::ContextAssembly;

/// Public provider selector for the query builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAiCompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderKind {
    Anthropic,
    OpenAiCompatible,
}

impl From<Provider> for ProviderKind {
    fn from(provider: Provider) -> Self {
        match provider {
            Provider::Anthropic => ProviderKind::Anthropic,
            Provider::OpenAiCompatible => ProviderKind::OpenAiCompatible,
        }
    }
}

/// Wrapper that redacts the value in Debug/Display.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SecretString(String);

impl SecretString {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedProvider {
    pub(crate) kind: ProviderKind,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) api_key: SecretString,
    pub(crate) max_context_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderRequest {
    pub(crate) assembly: ContextAssembly,
    pub(crate) model: String,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) temperature: Option<f32>,
    pub(crate) stream: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct ProviderUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: serde_json::Value,
    pub(crate) index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderFailureKind {
    Authentication,
    RateLimited,
    ContextTooLarge,
    InvalidRequest,
    ProviderUnavailable,
    Transport,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderFailure {
    pub(crate) kind: ProviderFailureKind,
    pub(crate) message: String,
    pub(crate) status: Option<u16>,
    pub(crate) provider_request_id: Option<String>,
    pub(crate) retryable: bool,
}
