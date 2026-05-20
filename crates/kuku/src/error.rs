use crate::provider::types::ProviderFailureKind;

#[derive(Debug, thiserror::Error)]
/// All error types produced by the kuku SDK.
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("time format error: {0}")]
    TimeFormat(#[from] time::error::Format),

    #[error("missing home directory; set KUKU_HOME")]
    MissingHomeDirectory,

    #[error("invalid KUKU_HOME: {0}")]
    InvalidKukuHome(String),

    #[error("invalid event stream: {0}")]
    InvalidEventStream(String),

    #[error("invalid session id: {0}")]
    InvalidSessionId(String),

    #[error("invalid workspace path: {0}")]
    InvalidWorkspacePath(String),

    #[error("provider error: {message}")]
    Provider {
        kind: ProviderFailureKind,
        message: String,
        provider: Option<String>,
        model: Option<String>,
    },

    #[error("missing provider configuration: {0}")]
    MissingProviderConfig(String),

    #[error("permission request not pending: {0}")]
    PermissionRequestNotPending(String),

    #[error("invalid policy file: {0}")]
    InvalidPolicy(String),

    #[error("prompt render error: {0}")]
    PromptRender(String),

    #[error("config: {0}")]
    ConfigLoad(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("session locked: {session} (holder pid {holder_pid})")]
    SessionLocked {
        session: std::path::PathBuf,
        holder_pid: i32,
    },

    #[error("child session requested permission: {tool} on {candidate}")]
    ChildPermissionRequested { tool: String, candidate: String },
}

/// Convenience alias for `std::result::Result<T, kuku::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Returns the wire error code for this error variant.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Io(_) | Error::Json(_) | Error::TimeFormat(_) => "internal",
            Error::MissingHomeDirectory
            | Error::InvalidKukuHome(_)
            | Error::InvalidEventStream(_)
            | Error::InvalidSessionId(_)
            | Error::InvalidWorkspacePath(_) => "internal",
            Error::Provider { kind, .. } => match kind {
                ProviderFailureKind::Authentication => "provider_auth",
                ProviderFailureKind::RateLimited => "provider_rate_limit",
                ProviderFailureKind::ContextTooLarge => "provider_overflow",
                ProviderFailureKind::InvalidRequest => "invalid_request",
                ProviderFailureKind::ProviderUnavailable => "provider_network",
                ProviderFailureKind::Transport => "provider_network",
                ProviderFailureKind::Unknown => "internal",
            },
            Error::MissingProviderConfig(_) => "internal",
            Error::PermissionRequestNotPending(_) => "internal",
            Error::InvalidPolicy(_) => "internal",
            Error::PromptRender(_) => "internal",
            Error::ConfigLoad(_) => "internal",
            Error::InvalidArgument(_) => "invalid_request",
            Error::SessionLocked { .. } => "session_locked",
            Error::ChildPermissionRequested { .. } => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use crate::provider::types::ProviderFailureKind;

    #[test]
    fn provider_error_variant_formats_message() {
        assert_eq!(
            Error::Provider {
                kind: ProviderFailureKind::ProviderUnavailable,
                message: "gateway timeout".to_string(),
                provider: Some("anthropic".to_string()),
                model: Some("claude-sonnet-4-6".to_string()),
            }
            .to_string(),
            "provider error: gateway timeout"
        );
    }

    #[test]
    fn missing_provider_config_variant_formats_message() {
        assert_eq!(
            Error::MissingProviderConfig("set KUKU_PROVIDER".to_string()).to_string(),
            "missing provider configuration: set KUKU_PROVIDER"
        );
    }

    #[test]
    fn permission_request_not_pending_variant_formats_message() {
        assert_eq!(
            Error::PermissionRequestNotPending("req_1".to_string()).to_string(),
            "permission request not pending: req_1"
        );
    }

    #[test]
    fn invalid_policy_variant_formats_message() {
        assert_eq!(
            Error::InvalidPolicy("malformed allow rule".to_string()).to_string(),
            "invalid policy file: malformed allow rule"
        );
    }

    #[test]
    fn prompt_render_variant_formats_message() {
        assert_eq!(
            Error::PromptRender("missing template variable: platform".to_string()).to_string(),
            "prompt render error: missing template variable: platform"
        );
    }
}
