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

    #[error("provider error: {0}")]
    Provider(String),

    #[error("missing provider configuration: {0}")]
    MissingProviderConfig(String),

    #[error("ambiguous provider configuration: {0}")]
    AmbiguousProviderConfig(String),

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
}

/// Convenience alias for `std::result::Result<T, kuku::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn provider_error_variant_formats_message() {
        assert_eq!(
            Error::Provider("gateway timeout".to_string()).to_string(),
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
    fn ambiguous_provider_config_variant_formats_message() {
        assert_eq!(
            Error::AmbiguousProviderConfig("select a provider explicitly".to_string()).to_string(),
            "ambiguous provider configuration: select a provider explicitly"
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
