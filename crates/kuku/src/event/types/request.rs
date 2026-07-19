use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{InteractionId, RequestId, RequestScope};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestCause {
    UserSubmission,
    ToolContinuation {
        parent_request_id: RequestId,
    },
    InteractionResume {
        interaction_id: InteractionId,
        parent_request_id: RequestId,
    },
    DelegatedAgent {
        parent_request_id: RequestId,
    },
    ReviewSubmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderFact {
    Anthropic,
    OpenAiCompatible,
    OpenAiResponses,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderUsage {
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub input_tokens: Option<u64>,
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub output_tokens: Option<u64>,
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub cached_input_tokens: Option<u64>,
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CurrencyCode {
    #[serde(rename = "USD")]
    Usd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DecimalCost {
    pub currency: CurrencyCode,
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub micros: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureKind {
    Authentication,
    RateLimited,
    ContextTooLarge,
    InvalidRequest,
    ProviderUnavailable,
    Transport,
    Internal,
    Unknown,
    ServerRestarted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderFailureFact {
    pub kind: ProviderFailureKind,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestStarted {
    pub scope: RequestScope,
    pub cause: RequestCause,
    pub provider: ProviderFact,
    pub model: String,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestCompleted {
    pub scope: RequestScope,
    pub usage: ProviderUsage,
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub elapsed_ms: Option<u64>,
    pub provider_request_id: Option<String>,
    pub cost: Option<DecimalCost>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestFailed {
    pub scope: RequestScope,
    pub usage: Option<ProviderUsage>,
    #[serde(deserialize_with = "super::task::deserialize_optional_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub elapsed_ms: Option<u64>,
    pub provider_request_id: Option<String>,
    pub cost: Option<DecimalCost>,
    pub failure: ProviderFailureFact,
}
