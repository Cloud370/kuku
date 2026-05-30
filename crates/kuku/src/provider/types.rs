use std::fmt;

use serde::{Deserialize, Serialize};

use crate::context::ContextAssembly;
use crate::prompt::PromptCatalog;

/// Wire format selector for provider API protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderFormat {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
}

impl ProviderFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderFormat::Anthropic => "anthropic",
            ProviderFormat::OpenAiChat => "openai-chat",
            ProviderFormat::OpenAiResponses => "openai-responses",
        }
    }

    pub fn parse_from_str(s: &str) -> std::result::Result<Self, &'static str> {
        match s {
            "anthropic" => Ok(ProviderFormat::Anthropic),
            "openai-chat" => Ok(ProviderFormat::OpenAiChat),
            "openai-responses" => Ok(ProviderFormat::OpenAiResponses),
            _ => Err("format must be anthropic/openai-chat/openai-responses"),
        }
    }
}

/// Public provider selector for the query builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAiCompatible,
    OpenAiResponses,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedProvider {
    pub(crate) kind: ProviderKind,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) api_key: SecretString,
    pub(crate) max_context_tokens: u32,
    pub(crate) max_output_tokens: u32,
    pub(crate) think_level: crate::config::ThinkLevel,
    pub(crate) thinking: crate::config::ResolvedThinking,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderRequest<'a> {
    pub(crate) assembly: ContextAssembly,
    pub(crate) catalog: &'a PromptCatalog,
    pub(crate) model: String,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) temperature: Option<f32>,
    pub(crate) stream: bool,
    pub(crate) think_level: crate::config::ThinkLevel,
    pub(crate) thinking: crate::config::ResolvedThinking,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
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
    pub(crate) retryable: bool,
}
