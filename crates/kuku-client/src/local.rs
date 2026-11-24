//! Discovery and process control for the per-user managed HTTP service.

mod build;
#[cfg(windows)]
mod windows;

pub use build::BuildPolicy;

use std::ffi::{OsStr, OsString};
use std::fs::{self, TryLockError};
use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use url::Url;
use wreq::{Method, Response, StatusCode};

#[cfg(not(windows))]
use tokio::process::Child as DetachedChild;

#[cfg(windows)]
use self::windows::{DetachedChild, spawn_detached};

/// Revision of the Session protocol expected by the product client.
pub const SESSION_PROTOCOL_VERSION: u32 = 2;

const MANAGEMENT_VERSION: u32 = 1;
const DISCOVERY_LIMIT: u64 = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Status and protocol revisions reported by an authenticated service.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServiceStatus {
    /// Build version of the running server.
    pub version: String,
    /// Revision of the Session protocol.
    pub protocol_version: u32,
    /// Revision of the independent service management API.
    pub management_version: u32,
    /// Identity of the managed process, when applicable.
    pub instance_id: Option<String>,
    /// Process ID of the managed service.
    pub pid: Option<u32>,
    /// Canonical server data directory, when reported.
    pub data_dir: Option<String>,
    /// Canonical configuration file used by the service.
    pub config_path: Option<String>,
    /// Active Agent work and cleanup holding the server open.
    pub busy: Vec<String>,
    /// Whether the server has paused admission to close Sessions.
    pub draining: bool,
}

/// Stable identity and configuration of an authenticated managed service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceIdentity {
    /// Loopback HTTP endpoint observed for this instance.
    pub endpoint: String,
    /// Unique identity assigned when the service starts.
    pub instance_id: String,
    /// Process ID of this instance.
    pub pid: u32,
    /// Canonical managed data directory.
    pub data_dir: String,
    /// Canonical configuration file used by this instance.
    pub config_path: PathBuf,
}

/// Failure to identify, contact or manage a local service.
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    /// The local filesystem or process operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The service URL or its access token was invalid.
    #[error("{0}")]
    Request(String),
    /// HTTP management transport failed.
    #[error("service management request failed: {0}")]
    Transport(#[source] wreq::Error),
    /// The service explicitly rejected a management operation.
    #[error("{message} ({code})")]
    Service {
        /// Stable service error code.
        code: String,
        /// Human-readable reason for the error.
        message: String,
    },
    /// Discovery failed to identify the running process.
    #[error("service discovery does not match the authenticated server in {0}")]
    Identity(PathBuf),
    /// The previously observed service was replaced or disappeared before restart.
    #[error(
        "the managed server changed before restart; inspect its current instance before retrying"
    )]
    ServiceChanged,
    /// The running build cannot satisfy the requested build policy.
    #[error(
        "kuku {running} is already running. Open that version to continue. This client is {requested}."
    )]
    BuildMismatch {
        /// Build in use by the running service.
        running: String,
        /// Build requested by the client.
        requested: String,
    },
    /// A build change must wait for current work to settle.
    #[error("Service is busy on {version}. Retry after work finishes to use kuku {requested}.", version = .running.version)]
    BuildDeferred {
        /// Service status and work blocking replacement.
        running: Box<ServiceStatus>,
        /// Build requested by the client.
        requested: String,
    },
    /// The service is using a different explicitly selected configuration.
    #[error(
        "the managed server is using {active}; requested {requested}. Run kuku server restart with the requested --config to switch"
    )]
    ConfigConflict {
        /// Path already in use.
        active: PathBuf,
        /// Path explicitly requested by this client.
        requested: PathBuf,
    },
    /// The selected configuration does not exist.
    #[error(
        "config file not found: {0}. Run kuku init to create the default config or select an existing --config"
    )]
    ConfigMissing(PathBuf),
    /// The service uses an unsupported Session protocol.
    #[error(
        "client Session protocol {client} differs from server {version} protocol {server}. Run kuku server status or kuku server restart"
    )]
    ProtocolVersion {
        /// Protocol required by the client.
        client: u32,
        /// Protocol reported by the service.
        server: u32,
        /// Build version reported by the service.
        version: String,
    },
    /// The running service does not speak the required management protocol.
    #[error("server management protocol {0} does not match client protocol {MANAGEMENT_VERSION}")]
    ManagementVersion(u32),
    /// The service process exited before publishing a verified endpoint.
    #[error("managed server exited during startup ({status}); check configuration at {config}")]
    StartupFailed {
        /// Exit status of the managed process.
        status: ExitStatus,
        /// Configuration supplied to the managed process.
        config: PathBuf,
    },
    /// A process transition did not finish in time.
    #[error("timed out {0} the managed server; run kuku server status")]
    TimedOut(&'static str),
}

/// Local service identity and management for one resolved data directory.
#[derive(Clone)]
pub struct LocalService {
    data_dir: PathBuf,
    config_path: PathBuf,
    executable: PathBuf,
    explicit_config: bool,
    http: wreq::Client,
}

impl LocalService {
    /// Resolve local paths and prepare authenticated loopback transport.
    pub fn resolve(data_dir: &Path, config: Option<&Path>) -> Result<Self, LocalError> {
        fs::create_dir_all(data_dir)?;
        let data_dir = data_dir.canonicalize()?;
        let default_config = data_dir.join("config.toml");
        let config_path = config.unwrap_or(&default_config);
        let config_path = match config_path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::path::absolute(config_path)?
            }
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            data_dir,
            config_path,
            executable: std::env::current_exe()?,
            explicit_config: config.is_some(),
            http: http_client()?,
        })
    }

    /// Return the canonical directory owning the service and its data.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return the candidate configuration path for a new managed service.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Attach to a compatible instance or launch it when absent.
    pub async fn connect(&self) -> Result<LocalConnection, LocalError> {
        let connection = match self.status().await? {
            Some(connection) if !connection.status.draining => connection,
            Some(_) | None => {
                let _launcher = self.acquire_launcher().await?;
                self.attach_or_start().await?
            }
        };
        self.compatible(connection)
    }

    fn check_config(&self, connection: &LocalConnection) -> Result<(), LocalError> {
        if self.explicit_config && self.config_path != connection.config_path {
            return Err(LocalError::ConfigConflict {
                active: connection.config_path.clone(),
                requested: self.config_path.clone(),
            });
        }
        Ok(())
    }

    fn compatible(&self, connection: LocalConnection) -> Result<LocalConnection, LocalError> {
        self.check_config(&connection)?;
        if connection.status.protocol_version != SESSION_PROTOCOL_VERSION {
            return Err(LocalError::ProtocolVersion {
                client: SESSION_PROTOCOL_VERSION,
                server: connection.status.protocol_version,
                version: connection.status.version,
            });
        }
        Ok(connection)
    }

    /// Authenticate and identify the current instance independently of the Session protocol.
    pub async fn status(&self) -> Result<Option<LocalConnection>, LocalError> {
        let directory = self.service_directory();
        if !validate_existing_directory(&directory)? || !instance_locked(&directory)? {
            return Ok(None);
        }
        let Some(record) = read_discovery(&directory)? else {
            return Ok(None);
        };
        if !record.address.ip().is_loopback()
            || record.address.port() == 0
            || Path::new(&record.data_dir) != self.data_dir
            || !Path::new(&record.config_path).is_absolute()
            || !valid_secret(&record.token)
            || !valid_secret(&record.instance_id)
        {
            return Err(LocalError::Identity(directory));
        }
        let endpoint = format!("http://{}", record.address);
        let status = get_status(&self.http, &endpoint, &record.token).await;
        if !instance_locked(&directory)? {
            return Ok(None);
        }
        let current = read_discovery(&directory)?;
        if current.is_none_or(|current| current.instance_id != record.instance_id) {
            return Ok(None);
        }
        let status = match status {
            Err(LocalError::Transport(error)) if error.is_connect() => return Ok(None),
            result => result?,
        };
        if status.instance_id.as_deref() != Some(record.instance_id.as_str())
            || status.pid != Some(record.pid)
            || status.data_dir.as_deref() != Some(self.data_dir.to_string_lossy().as_ref())
            || status.config_path.as_deref()
                != Some(Path::new(&record.config_path).to_string_lossy().as_ref())
        {
            return Err(LocalError::Identity(directory));
        }
        Ok(Some(LocalConnection {
            endpoint,
            token: record.token,
            address: record.address,
            config_path: PathBuf::from(record.config_path),
            status,
        }))
    }

    /// Replace an idle managed service or report the work preventing safe replacement.
    pub async fn restart(&self) -> Result<RestartOutcome, LocalError> {
        self.restart_expected(None).await
    }

    /// Restart only the observed instance, failing before stopping or launching a replacement if it changed.
    pub async fn restart_if_current(
        &self,
        expected: &ServiceIdentity,
    ) -> Result<RestartOutcome, LocalError> {
        self.restart_expected(Some(expected)).await
    }

    async fn restart_expected(
        &self,
        expected: Option<&ServiceIdentity>,
    ) -> Result<RestartOutcome, LocalError> {
        let _launcher = self.acquire_launcher().await?;
        let current = self.wait_for_live_instance().await?;
        if expected.is_some_and(|expected| {
            current
                .as_ref()
                .is_none_or(|current| current.identity() != *expected)
        }) {
            return Err(LocalError::ServiceChanged);
        }
        let Some(connection) = current else {
            let launched = self.start(&self.config_path, ephemeral_address()).await?;
            return Ok(RestartOutcome::Restarted(launched));
        };
        self.replace(connection).await
    }

    async fn replace(&self, connection: LocalConnection) -> Result<RestartOutcome, LocalError> {
        let config = if self.explicit_config {
            &self.config_path
        } else {
            &connection.config_path
        };
        require_config(config)?;
        let response = request(
            &self.http,
            Method::POST,
            connection.endpoint(),
            connection.token(),
            &["service", "restart"],
        )
        .await?;
        if response.status() == StatusCode::CONFLICT {
            let problem = response.json::<ServiceProblem>().await.map_err(transport)?;
            if problem.code == "service_busy" {
                let mut status = connection.status;
                status.busy = problem.busy;
                return Ok(RestartOutcome::Deferred(status));
            }
            return Err(problem.into_error());
        }
        if response.status() != StatusCode::ACCEPTED {
            return Err(response_error(response).await);
        }
        self.wait_for_exit().await?;
        let launched = self.start(config, connection.address).await?;
        if launched.config_path.as_path() != config.as_path() {
            return Err(LocalError::ConfigConflict {
                active: launched.config_path,
                requested: config.to_owned(),
            });
        }
        Ok(RestartOutcome::Restarted(launched))
    }

    fn service_directory(&self) -> PathBuf {
        self.data_dir.join("service")
    }

    async fn acquire_launcher(&self) -> Result<fslock::LockFile, LocalError> {
        let directory = self.service_directory();
        kuku::util::private_directory(&directory)?;
        let mut lock = fslock::LockFile::open(&directory.join("launcher.lock"))?;
        let start = Instant::now();
        while !lock.try_lock()? {
            pause(start, "waiting to launch").await?;
        }
        Ok(lock)
    }

    async fn wait_for_live_instance(&self) -> Result<Option<LocalConnection>, LocalError> {
        let directory = self.service_directory();
        let start = Instant::now();
        loop {
            match self.status().await? {
                Some(found) if !found.status.draining => return Ok(Some(found)),
                Some(_) | None => {}
            }
            if !instance_locked(&directory)? {
                return Ok(None);
            }
            pause(start, "waiting for a ready endpoint from").await?;
        }
    }

    /// Wait until this data directory's managed-service instance lease is released.
    ///
    /// This does not request shutdown or guarantee that the operating-system process has exited.
    pub async fn wait_for_exit(&self) -> Result<(), LocalError> {
        let directory = self.service_directory();
        let start = Instant::now();
        while instance_locked(&directory)? {
            pause(start, "waiting for shutdown of").await?;
        }
        Ok(())
    }

    async fn attach_or_start(&self) -> Result<LocalConnection, LocalError> {
        match self.wait_for_live_instance().await? {
            Some(connection) => Ok(connection),
            None => self.start(&self.config_path, ephemeral_address()).await,
        }
    }

    async fn start(
        &self,
        config: &Path,
        address: SocketAddr,
    ) -> Result<LocalConnection, LocalError> {
        require_config(config)?;
        let mut child = launch_child(&self.executable, &self.data_dir, config, address)?;
        let start = Instant::now();
        loop {
            if let Some(ready) = self.status().await? {
                return Ok(ready);
            }
            if let Some(status) = child.try_wait()? {
                return Err(LocalError::StartupFailed {
                    status,
                    config: config.to_path_buf(),
                });
            }
            pause(start, "starting").await?;
        }
    }
}

/// Authenticated endpoint and reported status for one managed service instance.
#[derive(Clone)]
pub struct LocalConnection {
    endpoint: String,
    token: String,
    address: SocketAddr,
    config_path: PathBuf,
    status: ServiceStatus,
}

impl LocalConnection {
    /// Return the loopback HTTP URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Return the native access token for this instance.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Return the service status observed by discovery.
    pub fn status(&self) -> &ServiceStatus {
        &self.status
    }

    /// Return the stable identity verified during authenticated discovery.
    pub fn identity(&self) -> ServiceIdentity {
        ServiceIdentity {
            endpoint: self.endpoint.clone(),
            instance_id: self.status.instance_id.clone().unwrap(),
            pid: self.status.pid.unwrap(),
            data_dir: self.status.data_dir.clone().unwrap(),
            config_path: self.config_path.clone(),
        }
    }
}

/// Result of asking a managed service to release its Sessions and restart.
pub enum RestartOutcome {
    /// The active instance has work that must finish before replacement.
    Deferred(ServiceStatus),
    /// A replacement process has published an authenticated endpoint.
    Restarted(LocalConnection),
}

/// Read authenticated management status from the explicitly specified HTTP endpoint.
pub async fn service_status(endpoint: &str, token: &str) -> Result<ServiceStatus, LocalError> {
    get_status(&http_client()?, endpoint, token).await
}

#[derive(Deserialize)]
struct Discovery {
    address: SocketAddr,
    token: String,
    instance_id: String,
    pid: u32,
    config_path: OsString,
    data_dir: OsString,
}

#[derive(Deserialize)]
struct ServiceProblem {
    code: String,
    message: String,
    #[serde(default)]
    busy: Vec<String>,
}

impl ServiceProblem {
    fn into_error(self) -> LocalError {
        LocalError::Service {
            code: self.code,
            message: self.message,
        }
    }
}

fn validate_existing_directory(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            kuku::util::private_directory(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn instance_locked(directory: &Path) -> io::Result<bool> {
    let lock = match fs::File::open(directory.join("instance.lock")) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match lock.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn read_discovery(directory: &Path) -> Result<Option<Discovery>, LocalError> {
    let path = directory.join("connection.json");
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            if fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
                || !instance_locked(directory)?
            {
                return Ok(None);
            }
            return Err(error.into());
        }
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(DISCOVERY_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > DISCOVERY_LIMIT {
        return Err(LocalError::Identity(directory.to_path_buf()));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| LocalError::Identity(directory.to_path_buf()))
}

fn require_config(config: &Path) -> Result<(), LocalError> {
    match fs::metadata(config) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(LocalError::Request(format!(
            "config is not a file: {}",
            config.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(LocalError::ConfigMissing(config.to_path_buf()))
        }
        Err(error) => Err(error.into()),
    }
}

fn valid_secret(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn ephemeral_address() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

async fn pause(start: Instant, phase: &'static str) -> Result<(), LocalError> {
    if start.elapsed() >= STARTUP_TIMEOUT {
        return Err(LocalError::TimedOut(phase));
    }
    tokio::time::sleep(POLL_INTERVAL).await;
    Ok(())
}

fn launch_child(
    executable: &Path,
    data_dir: &Path,
    config: &Path,
    address: SocketAddr,
) -> io::Result<DetachedChild> {
    let listen = address.to_string();
    let args = [
        OsStr::new("__managed-server"),
        OsStr::new("--data-dir"),
        data_dir.as_os_str(),
        OsStr::new("--config"),
        config.as_os_str(),
        OsStr::new("--listen"),
        OsStr::new(&listen),
    ];
    spawn_detached(executable, &args, data_dir)
}

#[cfg(unix)]
fn spawn_detached(
    executable: &Path,
    args: &[&OsStr],
    data_dir: &Path,
) -> io::Result<DetachedChild> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    use tokio::process::Command;

    let mut command = Command::new(executable);
    command
        .args(args)
        .env("KUKU_HOME", data_dir)
        .current_dir(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: setsid is async-signal-safe and only changes the child launched for this service.
    // Rust's process API cannot create a detached session on Unix.
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn()
}

#[cfg(not(any(unix, windows)))]
fn spawn_detached(
    _executable: &Path,
    _args: &[&OsStr],
    _data_dir: &Path,
) -> io::Result<DetachedChild> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "managed server launch is unavailable on this platform",
    ))
}

fn http_client() -> Result<wreq::Client, LocalError> {
    wreq::Client::builder()
        .no_proxy()
        .redirect(wreq::redirect::Policy::none())
        .retry(wreq::retry::Policy::never())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(transport)
}

async fn get_status(
    http: &wreq::Client,
    endpoint: &str,
    token: &str,
) -> Result<ServiceStatus, LocalError> {
    let response = request(http, Method::GET, endpoint, token, &["service", "status"]).await?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let status: ServiceStatus = response.json().await.map_err(transport)?;
    if status.management_version != MANAGEMENT_VERSION {
        return Err(LocalError::ManagementVersion(status.management_version));
    }
    Ok(status)
}

async fn request(
    http: &wreq::Client,
    method: Method,
    endpoint: &str,
    token: &str,
    route: &[&str],
) -> Result<Response, LocalError> {
    let mut url = parse_endpoint(endpoint)?;
    url.path_segments_mut()
        .expect("HTTP(S) URLs have path segments")
        .pop_if_empty()
        .extend(route);
    if token.trim().is_empty() {
        return Err(LocalError::Request(
            "server access token is required".into(),
        ));
    }
    let timeout = if method == Method::POST {
        STARTUP_TIMEOUT
    } else {
        REQUEST_TIMEOUT
    };
    http.request(method, url.as_str())
        .bearer_auth(token)
        .timeout(timeout)
        .send()
        .await
        .map_err(transport)
}

fn parse_endpoint(endpoint: &str) -> Result<Url, LocalError> {
    let invalid = || {
        LocalError::Request(
            "expected an HTTP(S) service URL without credentials, query or fragment".into(),
        )
    };
    let url = Url::parse(endpoint).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url)
}

async fn response_error(response: Response) -> LocalError {
    let status = response.status();
    match response.json::<ServiceProblem>().await {
        Ok(problem) => problem.into_error(),
        Err(_) => LocalError::Request(format!("service management request returned HTTP {status}")),
    }
}

fn transport(error: wreq::Error) -> LocalError {
    LocalError::Transport(error.without_uri())
}

#[cfg(test)]
mod tests;
