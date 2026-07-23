use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kuku::config::SecretString;

use crate::api::{ApiError, ApiErrorCode, AuthMode, AuthStatus};

use super::write_private_atomic;

const TOKEN_DIGITS: usize = 6;
const TOKEN_SPACE: u32 = 1_000_000;
const LEGACY_TOKEN_HEX_BYTES: usize = 64;
const AUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const PER_IP_FAILURE_LIMIT: u32 = 10;
const GLOBAL_FAILURE_LIMIT: u32 = 100;

/// Identifies how the active bearer token was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BearerTokenSource {
    /// The token was read from an operator-supplied file.
    ExplicitFile,
    /// The token was read from the private server home.
    Persisted,
    /// The token was generated and persisted during this open.
    Generated,
}

/// Owns the bearer token without exposing it through formatting.
pub struct BearerTokenStore {
    token: SecretString,
    source: BearerTokenSource,
    rate_limiter: Mutex<AuthRateLimiter>,
}

impl BearerTokenStore {
    pub fn from_token(token: String) -> Result<Arc<Self>, ApiError> {
        let token = token.trim_end_matches(['\r', '\n']).to_owned();
        if !valid_token(&token) {
            return Err(internal_error("bearer credential has invalid format"));
        }
        Ok(Arc::new(Self {
            token: SecretString::new(token),
            source: BearerTokenSource::ExplicitFile,
            rate_limiter: Mutex::new(AuthRateLimiter::new()),
        }))
    }

    /// Loads an explicit or persisted token, generating one when absent.
    pub fn open(kuku_home: &Path, explicit_file: Option<&Path>) -> Result<Arc<Self>, ApiError> {
        if let Some(path) = explicit_file {
            return read_token(path, BearerTokenSource::ExplicitFile);
        }

        let token_path = kuku_home.join("auth").join("token");
        if token_path.exists() {
            return read_token(&token_path, BearerTokenSource::Persisted);
        }

        create_private_directory(&token_path)?;
        let token = generate_token()?;
        write_private_atomic(&token_path, token.as_bytes()).map_err(|error| {
            internal_error(format!("failed to persist bearer credential: {error}"))
        })?;
        Ok(Arc::new(Self {
            token: SecretString::new(token),
            source: BearerTokenSource::Generated,
            rate_limiter: Mutex::new(AuthRateLimiter::new()),
        }))
    }

    /// Authorizes a request using a bearer header or explicit loopback trust.
    pub fn authorize(
        &self,
        policy: &AuthPolicy,
        peer: SocketAddr,
        authorization: Option<&str>,
    ) -> Result<AuthContext, ApiError> {
        if let Some(header) = authorization {
            let now = Instant::now();
            if self.rate_limiter.lock().unwrap().is_blocked(peer.ip(), now) {
                return Err(auth_required());
            }
            let authenticated = parse_bearer(header)
                .map(|supplied| {
                    constant_time_equal(self.token.expose().as_bytes(), supplied.as_bytes())
                })
                .unwrap_or(false);
            if !authenticated {
                self.rate_limiter
                    .lock()
                    .unwrap()
                    .record_failure(peer.ip(), now);
                return Err(auth_required());
            }
            self.rate_limiter.lock().unwrap().clear(peer.ip());
            return Ok(AuthContext {
                authenticated: true,
                mode: AuthMode::Bearer,
            });
        }

        if policy.loopback_trust && peer.ip().is_loopback() {
            Ok(AuthContext {
                authenticated: true,
                mode: AuthMode::LoopbackTrusted,
            })
        } else {
            Err(auth_required())
        }
    }

    /// Maps request authentication into the canonical public status.
    pub fn status(&self, context: &AuthContext) -> AuthStatus {
        AuthStatus {
            authenticated: context.authenticated,
            mode: context.mode,
        }
    }

    /// Exposes the token only for the authorized terminal bootstrap boundary.
    pub fn expose_for_terminal(&self) -> &str {
        self.token.expose()
    }

    /// Returns the source used for the active token.
    pub fn source(&self) -> BearerTokenSource {
        self.source
    }
}

impl fmt::Debug for BearerTokenStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BearerTokenStore")
            .field("token", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

impl fmt::Display for BearerTokenStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerTokenStore(<redacted>)")
    }
}

/// Describes the authenticated state of one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthContext {
    /// Whether the request passed authentication.
    pub authenticated: bool,
    /// The authentication mode used by the request.
    pub mode: AuthMode,
}

/// Configures whether direct loopback requests may omit bearer auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthPolicy {
    /// Allows unauthenticated direct loopback peers when true.
    pub loopback_trust: bool,
}

struct FailureWindow {
    started_at: Instant,
    failures: u32,
}

impl FailureWindow {
    fn new(now: Instant) -> Self {
        Self {
            started_at: now,
            failures: 0,
        }
    }

    fn refresh(&mut self, now: Instant) {
        if now.duration_since(self.started_at) >= AUTH_FAILURE_WINDOW {
            self.started_at = now;
            self.failures = 0;
        }
    }
}

struct AuthRateLimiter {
    global: FailureWindow,
    per_ip: HashMap<IpAddr, FailureWindow>,
}

impl AuthRateLimiter {
    fn new() -> Self {
        Self {
            global: FailureWindow::new(Instant::now()),
            per_ip: HashMap::new(),
        }
    }

    fn is_blocked(&mut self, ip: IpAddr, now: Instant) -> bool {
        self.refresh(now);
        self.global.failures >= GLOBAL_FAILURE_LIMIT
            || self
                .per_ip
                .get(&ip)
                .is_some_and(|window| window.failures >= PER_IP_FAILURE_LIMIT)
    }

    fn record_failure(&mut self, ip: IpAddr, now: Instant) {
        self.refresh(now);
        self.global.failures += 1;
        self.per_ip
            .entry(ip)
            .or_insert_with(|| FailureWindow::new(now))
            .failures += 1;
    }

    fn clear(&mut self, ip: IpAddr) {
        self.per_ip.remove(&ip);
    }

    fn refresh(&mut self, now: Instant) {
        self.global.refresh(now);
        self.per_ip
            .retain(|_, window| now.duration_since(window.started_at) < AUTH_FAILURE_WINDOW);
    }
}

fn read_token(path: &Path, source: BearerTokenSource) -> Result<Arc<BearerTokenStore>, ApiError> {
    let bytes = fs::read(path)
        .map_err(|error| internal_error(format!("failed to read bearer credential: {error}")))?;
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| internal_error("bearer credential file is not UTF-8"))?
        .trim_end_matches(['\r', '\n']);
    if !valid_token(value) {
        return Err(internal_error("bearer credential file has invalid format"));
    }
    Ok(Arc::new(BearerTokenStore {
        token: SecretString::new(value),
        source,
        rate_limiter: Mutex::new(AuthRateLimiter::new()),
    }))
}

fn create_private_directory(path: &Path) -> Result<(), ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| internal_error("bearer credential path has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| {
        internal_error(format!(
            "failed to create bearer credential directory: {error}"
        ))
    })?;
    set_private_directory_permissions(parent)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ApiError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        internal_error(format!(
            "failed to protect bearer credential directory: {error}"
        ))
    })
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), ApiError> {
    Ok(())
}

fn generate_token() -> Result<String, ApiError> {
    let sample_limit = u32::MAX - (u32::MAX % TOKEN_SPACE);
    loop {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes).map_err(|error| {
            internal_error(format!("failed to generate bearer credential: {error}"))
        })?;
        let sample = u32::from_le_bytes(bytes);
        if sample < sample_limit {
            return Ok(format!("{:0TOKEN_DIGITS$}", sample % TOKEN_SPACE));
        }
    }
}

fn valid_token(value: &str) -> bool {
    (value.len() == TOKEN_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit()))
        || (value.len() == LEGACY_TOKEN_HEX_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

fn parse_bearer(header: &str) -> Option<&str> {
    let (scheme, value) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || !valid_token(value) {
        return None;
    }
    Some(value)
}

fn constant_time_equal(expected: &[u8], supplied: &[u8]) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    let compared_len = expected.len().max(supplied.len());
    for index in 0..compared_len {
        let expected_byte = expected.get(index).copied().unwrap_or_default();
        let supplied_byte = supplied.get(index).copied().unwrap_or_default();
        difference |= usize::from(expected_byte ^ supplied_byte);
    }
    difference == 0
}

fn auth_required() -> ApiError {
    ApiError::new(
        ApiErrorCode::AuthRequired,
        "bearer authentication required",
        "platform-auth",
    )
}

fn internal_error(message: impl Into<String>) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, message, "platform-auth")
}
