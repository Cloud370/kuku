//! HTTP status and structured error envelopes for the interactive protocol.

use std::io::ErrorKind;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub(crate) type Result<T> = std::result::Result<T, ApiError>;

#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub(crate) fn session_not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "session_not_found",
            "Session not found",
        )
    }

    pub(crate) fn receipt_not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "receipt_not_found",
            "Command has no committed receipt",
        )
    }

    pub(crate) fn value(&self) -> serde_json::Value {
        json!({"ok": false, "code": self.code, "message": self.message})
    }
}

impl From<kuku::Error> for ApiError {
    fn from(error: kuku::Error) -> Self {
        use kuku::Error;

        let (status, code) = match &error {
            Error::InvalidArgument(_)
            | Error::InvalidSessionId(_)
            | Error::InvalidWorkspacePath(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Error::CommandConflict(_) => (StatusCode::CONFLICT, "command_conflict"),
            Error::InputNotQueued(_) => (StatusCode::CONFLICT, "input_not_queued"),
            Error::SessionCreationConflict(_) => {
                (StatusCode::CONFLICT, "session_creation_conflict")
            }
            Error::ExecutionNotActive(_) => (StatusCode::CONFLICT, "execution_not_active"),
            Error::OutputUnavailable(_) => (StatusCode::NOT_FOUND, error.code()),
            Error::OutputQuotaExceeded { .. } => (StatusCode::INSUFFICIENT_STORAGE, error.code()),
            Error::PermissionRequestNotPending(_) | Error::QuestionRequestNotPending(_) => {
                (StatusCode::CONFLICT, "reply_not_pending")
            }
            Error::SessionLocked { .. }
            | Error::InterruptedOpenTool(_)
            | Error::JobCleanupReviewRequired(_) => (StatusCode::CONFLICT, error.code()),
            Error::MissingProviderConfig(_) | Error::ConfigLoad(_) => {
                (StatusCode::BAD_REQUEST, "model_configuration")
            }
            Error::Io(inner) if inner.kind() == ErrorKind::NotFound => {
                (StatusCode::NOT_FOUND, "not_found")
            }
            Error::Io(inner) if inner.kind() == ErrorKind::InvalidInput => {
                (StatusCode::BAD_REQUEST, "invalid_request")
            }
            Error::Io(inner) if inner.kind() == ErrorKind::PermissionDenied => {
                (StatusCode::FORBIDDEN, "permission_denied")
            }
            _ => (StatusCode::INTERNAL_SERVER_ERROR, error.code()),
        };
        Self::new(status, code, error.to_string())
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        kuku::Error::from(error).into()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.value())).into_response()
    }
}
