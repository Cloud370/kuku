use std::collections::BTreeSet;

use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY, VARY, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};

use crate::api::{ApiError, ApiErrorCode};

use super::AuthContext;

const X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::from_static("x-content-type-options");
const PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");

/// Enforces exact browser origins configured for the active listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginPolicy {
    allowed: BTreeSet<String>,
}

impl OriginPolicy {
    /// Validates and freezes the exact list of allowed origins.
    pub fn new(allowed: Vec<String>) -> Result<Self, ApiError> {
        let mut validated = BTreeSet::new();
        for origin in allowed {
            validate_origin(&origin)?;
            validated.insert(origin);
        }
        Ok(Self { allowed: validated })
    }

    /// Rejects a supplied origin unless it exactly matches the allowlist.
    pub fn check<'a>(&self, origin: Option<&'a str>) -> Result<Option<&'a str>, ApiError> {
        match origin {
            Some(value) if self.allowed.contains(value) => Ok(Some(value)),
            Some(_) => Err(ApiError::new(
                ApiErrorCode::OriginNotAllowed,
                "request origin is not allowed",
                "platform-origin",
            )),
            None => Ok(None),
        }
    }

    /// Checks an Origin only after the request has authenticated.
    pub fn check_request<'a>(
        &self,
        origin: Option<&'a str>,
        context: &AuthContext,
    ) -> Result<Option<&'a str>, ApiError> {
        if !context.authenticated {
            return Err(ApiError::new(
                ApiErrorCode::AuthRequired,
                "bearer authentication required",
                "platform-origin",
            ));
        }
        match context.mode {
            crate::api::AuthMode::LoopbackTrusted | crate::api::AuthMode::Bearer => {
                self.check(origin)
            }
        }
    }

    /// Returns the validated origins for CSP construction.
    pub fn connect_origins(&self) -> Vec<String> {
        self.allowed.iter().cloned().collect()
    }
}

/// Applies the browser and API response security policy.
pub struct SecurityHeaders;

impl SecurityHeaders {
    /// Applies restrictive browser headers and disables API caching.
    pub fn apply(headers: &mut HeaderMap, connect_origins: &[String]) {
        let connect = if connect_origins.is_empty() {
            "connect-src 'self'".to_owned()
        } else {
            format!("connect-src 'self' {}", connect_origins.join(" "))
        };
        let csp = format!(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; {connect}; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"
        );

        insert_header(headers, CONTENT_SECURITY_POLICY, &csp);
        insert_header(headers, X_CONTENT_TYPE_OPTIONS, "nosniff");
        insert_header(headers, REFERRER_POLICY, "no-referrer");
        insert_header(
            headers,
            PERMISSIONS_POLICY,
            "camera=(), microphone=(), geolocation=()",
        );
        insert_header(headers, VARY, "Origin");
        insert_header(headers, CACHE_CONTROL, "no-store");
    }

    /// Maps an authentication failure to HTTP 401 with its bearer challenge.
    pub fn apply_auth_failure(headers: &mut HeaderMap, error: &ApiError) -> StatusCode {
        if error.code() == ApiErrorCode::AuthRequired {
            insert_header(headers, WWW_AUTHENTICATE, "Bearer");
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::FORBIDDEN
        }
    }
}

fn validate_origin(value: &str) -> Result<(), ApiError> {
    let uri = value.parse::<Uri>().map_err(|_| invalid_origin())?;
    let scheme = uri.scheme_str().ok_or_else(invalid_origin)?;
    let authority = uri.authority().ok_or_else(invalid_origin)?;
    if !matches!(scheme, "http" | "https")
        || authority.as_str().contains('@')
        || value != format!("{scheme}://{authority}")
    {
        return Err(invalid_origin());
    }
    Ok(())
}

fn invalid_origin() -> ApiError {
    ApiError::new(
        ApiErrorCode::InvalidRequest,
        "allowed origin must be an exact HTTP or HTTPS origin",
        "platform-origin",
    )
}

fn insert_header(headers: &mut HeaderMap, name: HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}
