use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ApiVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    InvalidRequest,
    AuthRequired,
    Forbidden,
    OriginNotAllowed,
    InitIncomplete,
    StaleServerRevision,
    WorkspaceNotFound,
    WorkspaceInUse,
    WorkspaceUnavailable,
    TaskNotFound,
    RequestNotFound,
    ConversationNotFound,
    FileNotFound,
    TaskBusy,
    StaleCommand,
    IdempotencyConflict,
    RunNotActive,
    InteractionNotPending,
    CursorAhead,
    Outdated,
    PayloadTooLarge,
    UnsupportedMediaType,
    StreamLimit,
    ServerBusy,
    ProviderUnavailable,
    StorageExhausted,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApiError {
    pub api_version: ApiVersion,
    pub code: ApiErrorCode,
    pub message: String,
    pub trace_id: String,
    #[schemars(required)]
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub details: Option<serde_json::Value>,
}

fn deserialize_required_nullable<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::deserialize(deserializer)
}

impl ApiError {
    pub fn new(
        code: ApiErrorCode,
        message: impl Into<String>,
        trace_id: impl Into<String>,
    ) -> Self {
        Self {
            api_version: ApiVersion,
            code,
            message: message.into(),
            trace_id: trace_id.into(),
            details: None,
        }
    }

    pub fn task_busy(trace_id: impl Into<String>) -> Self {
        Self::new(
            ApiErrorCode::TaskBusy,
            "task already has an active run",
            trace_id,
        )
    }

    pub fn code(&self) -> ApiErrorCode {
        self.code
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}
