pub mod api;
pub mod config_watcher;
pub mod platform;
pub mod routes;
pub mod run_manager;
pub mod server_args;
mod server_limits;
#[cfg(feature = "test-scenarios")]
pub mod testing;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use kuku::event::WorkspaceId;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use platform::{AuthPolicy, BearerTokenStore, ProviderProbe};
use run_manager::driver::RunDriverFactory;
use run_manager::{ReviewSubmissionValidator, SkillSelectionValidator};
pub use server_limits::ServerLimits;

pub struct AppState {
    pub platform: Arc<platform::PlatformServices>,
    pub task_runtime: Arc<run_manager::TaskRuntime>,
    pub kuku_home: PathBuf,
    pub limits: ServerLimits,
    _instance_lock: platform::ServerInstanceLock,
}

pub struct PreparedServer {
    pub app: Router,
    pub listener: tokio::net::TcpListener,
    pub state: Arc<AppState>,
    watcher: Option<config_watcher::ConfigWatcherHandle>,
}

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("KUKU_HOME already has a running web server")]
    AlreadyRunning,
    #[error("invalid server argument: {0}")]
    InvalidArgument(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("platform startup failed: {0:?}")]
    Platform(crate::api::ApiError),
}

impl PreparedServer {
    pub fn print_connection_info(&self, show_qr: bool) {
        let token = self.state.platform.auth.expose_for_terminal();
        let origin = self.state.platform.connection.local_url.clone();
        let url = format!("{origin}/#credential={token}");
        println!("kuku server: {origin}");
        println!("kuku credential URL: {url}");
        if let Some(lan) = &self.state.platform.connection.lan_url {
            println!("kuku LAN: {lan}");
            println!("kuku LAN credential URL: {lan}/#credential={token}");
        }
        if show_qr {
            if let Ok(code) = qrcode::QrCode::new(url.as_bytes()) {
                println!(
                    "{}",
                    code.render::<qrcode::render::unicode::Dense1x2>().build()
                );
            }
        }
    }

    pub async fn serve(self) -> Result<(), StartupError> {
        let app = self.app.clone();
        self.serve_with(app).await
    }

    pub async fn serve_with(mut self, app: Router) -> Result<(), StartupError> {
        let listener = self.listener;
        let state = Arc::clone(&self.state);
        let watcher = self.watcher.take();
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
        state.task_runtime.shutdown().await;
        if let Some(watcher) = watcher {
            watcher.shutdown().await;
        }
        result.map_err(StartupError::Io)
    }
}

pub async fn prepare_server(args: server_args::ServerArgs) -> Result<PreparedServer, StartupError> {
    let listen_addr: SocketAddr = args.listen.parse().map_err(|error| {
        StartupError::InvalidArgument(format!("invalid listen address: {error}"))
    })?;
    let limits = ServerLimits::with_max_concurrent_runs(args.max_concurrent_runs)
        .map_err(StartupError::InvalidArgument)?;
    let home = std::env::var_os("KUKU_HOME")
        .map(PathBuf::from)
        .or_else(|| home::home_dir().map(|dir| dir.join(".kuku")))
        .unwrap_or_else(|| PathBuf::from(".kuku"));
    let config_path = args.config.unwrap_or_else(|| home.join("config.toml"));
    let token = args
        .auth_token_file
        .as_deref()
        .map(std::fs::read_to_string)
        .transpose()?;
    let mut registration_roots = args
        .registration_root
        .into_iter()
        .map(|value| {
            let (label, path) = value.split_once('=').ok_or_else(|| {
                StartupError::InvalidArgument("registration root must use LABEL=PATH".to_owned())
            })?;
            if label.trim().is_empty() || path.trim().is_empty() {
                return Err(StartupError::InvalidArgument(
                    "registration root label and path are required".to_owned(),
                ));
            }
            Ok(platform::RegistrationRootSpec {
                label: label.to_owned(),
                path: PathBuf::from(path),
            })
        })
        .collect::<Result<Vec<_>, StartupError>>()?;
    if registration_roots.is_empty() {
        registration_roots.push(platform::RegistrationRootSpec {
            label: "Current directory".to_owned(),
            path: std::env::current_dir()?,
        });
    }
    let explicit_origins = args.allow_origin;
    let instance_lock = platform::ServerInstanceLock::acquire(&home).map_err(|error| {
        if matches!(error, platform::InstanceLockError::AlreadyRunning) {
            StartupError::AlreadyRunning
        } else {
            StartupError::Platform(crate::api::ApiError::new(
                crate::api::ApiErrorCode::Internal,
                error.to_string(),
                "server-lock",
            ))
        }
    })?;
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    let advertised = advertised_origins(
        listener.local_addr()?,
        if_addrs::get_if_addrs()?
            .into_iter()
            .map(|interface| interface.ip()),
    );
    let mut allowed_origins = advertised.all();
    for origin in explicit_origins {
        if !allowed_origins.contains(&origin) {
            allowed_origins.push(origin);
        }
    }
    let state = AppState::open_with_existing_lock(
        &home,
        &config_path,
        token,
        registration_roots,
        advertised.local,
        advertised.lan.first().cloned(),
        allowed_origins,
        limits,
        instance_lock,
    )
    .await
    .map_err(|error| {
        if error.message == "another web server is already running" {
            StartupError::AlreadyRunning
        } else {
            StartupError::Platform(error)
        }
    })?;
    let watcher =
        config_watcher::ConfigWatcherHandle::start(config_path, Arc::clone(&state.platform.config));
    let app = build_app(Arc::clone(&state));
    Ok(PreparedServer {
        app,
        listener,
        state,
        watcher: Some(watcher),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedOrigins {
    pub local: String,
    pub lan: Vec<String>,
}

impl AdvertisedOrigins {
    pub fn all(&self) -> Vec<String> {
        let mut origins = vec![self.local.clone()];
        for origin in &self.lan {
            if !origins.contains(origin) {
                origins.push(origin.clone());
            }
        }
        origins
    }
}

pub fn advertised_origins(
    bound: SocketAddr,
    interface_ips: impl IntoIterator<Item = std::net::IpAddr>,
) -> AdvertisedOrigins {
    let port = bound.port();
    if !bound.ip().is_unspecified() {
        let origin = origin_for(bound.ip(), port);
        return AdvertisedOrigins {
            local: origin.clone(),
            lan: (!bound.ip().is_loopback())
                .then_some(origin)
                .into_iter()
                .collect(),
        };
    }
    let local_ip = if bound.is_ipv4() {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    } else {
        std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
    };
    let mut lan = interface_ips
        .into_iter()
        .filter(|ip| ip.is_ipv4() == bound.is_ipv4())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
        .filter(|ip| !matches!(ip, std::net::IpAddr::V6(value) if value.is_unicast_link_local()))
        .map(|ip| origin_for(ip, port))
        .collect::<Vec<_>>();
    lan.sort();
    lan.dedup();
    AdvertisedOrigins {
        local: origin_for(local_ip, port),
        lan,
    }
}

fn origin_for(ip: std::net::IpAddr, port: u16) -> String {
    format!("http://{}", SocketAddr::new(ip, port))
}

async fn auth_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    mut request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if request.uri().path() == "/health" {
        return secure_response(&state, next.run(request).await);
    }

    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let context = match state.platform.auth.authorize(
        &AuthPolicy {
            loopback_trust: false,
        },
        addr,
        supplied,
    ) {
        Ok(context) => context,
        Err(_) => return secure_response(&state, auth_required_response()),
    };
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if let Err(error) = state.platform.origin_policy.check_request(origin, &context) {
        let response = (StatusCode::FORBIDDEN, Json(error)).into_response();
        return secure_response(&state, response);
    }
    request.extensions_mut().insert(context);
    if !state.platform.bootstrap.status().await.complete
        && initialized_route(request.method(), request.uri().path())
    {
        let error = crate::api::ApiError::new(
            crate::api::ApiErrorCode::InitIncomplete,
            "server initialization is incomplete",
            "platform-init-gate",
        );
        return secure_response(&state, (StatusCode::CONFLICT, Json(error)).into_response());
    }
    secure_response(&state, next.run(request).await)
}

fn initialized_route(_method: &Method, path: &str) -> bool {
    matches!(
        path,
        "/api/v1/settings" | "/api/v1/workspaces" | "/api/v1/catalog"
    ) || path.starts_with("/api/v1/workspaces/")
        || path == "/api/v1/tasks"
        || path.starts_with("/api/v1/tasks/")
}

pub fn build_app(state: Arc<AppState>) -> Router {
    let health_state = state.clone();
    let api = platform::router(state.platform.clone())
        .merge(routes::tasks::router(state.task_runtime.clone()));
    Router::new()
        .route(
            "/health",
            axum::routing::get(move || routes::health::health(health_state.clone())),
        )
        .nest("/api/v1", api)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(state.limits.http_body_bytes))
}

fn auth_required_response() -> Response {
    let error = crate::api::ApiError::new(
        crate::api::ApiErrorCode::AuthRequired,
        "bearer authentication required",
        "platform-auth",
    );
    let mut response = (StatusCode::UNAUTHORIZED, Json(error)).into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        axum::http::HeaderValue::from_static("Bearer"),
    );
    response
}

fn secure_response(state: &AppState, mut response: Response) -> Response {
    platform::SecurityHeaders::apply(
        response.headers_mut(),
        &state.platform.origin_policy.connect_origins(),
    );
    response
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub async fn open_with_config_path(
        home: &std::path::Path,
        config_path: &std::path::Path,
        bearer_token: Option<String>,
        registration_roots: Vec<platform::RegistrationRootSpec>,
        preferred_origin: String,
        limits: ServerLimits,
    ) -> Result<Arc<Self>, crate::api::ApiError> {
        Self::open_with_config_path_and_origins(
            home,
            config_path,
            bearer_token,
            registration_roots,
            preferred_origin.clone(),
            None,
            vec![preferred_origin],
            limits,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn open_with_config_path_and_origins(
        home: &std::path::Path,
        config_path: &std::path::Path,
        bearer_token: Option<String>,
        registration_roots: Vec<platform::RegistrationRootSpec>,
        preferred_origin: String,
        lan_origin: Option<String>,
        allowed_origins: Vec<String>,
        limits: ServerLimits,
    ) -> Result<Arc<Self>, crate::api::ApiError> {
        let instance_lock = platform::ServerInstanceLock::acquire(home).map_err(|error| {
            crate::api::ApiError::new(
                crate::api::ApiErrorCode::Internal,
                error.to_string(),
                "server-lock",
            )
        })?;
        Self::open_with_existing_lock(
            home,
            config_path,
            bearer_token,
            registration_roots,
            preferred_origin,
            lan_origin,
            allowed_origins,
            limits,
            instance_lock,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_with_existing_lock(
        home: &std::path::Path,
        config_path: &std::path::Path,
        bearer_token: Option<String>,
        registration_roots: Vec<platform::RegistrationRootSpec>,
        preferred_origin: String,
        lan_origin: Option<String>,
        allowed_origins: Vec<String>,
        limits: ServerLimits,
        instance_lock: platform::ServerInstanceLock,
    ) -> Result<Arc<Self>, crate::api::ApiError> {
        let revisions = platform::ServerRevisionCoordinator::open(home);
        let config =
            platform::ConfigService::open(config_path.to_owned(), Arc::clone(&revisions)).await?;
        let repository = run_manager::TaskRepository::open(home)
            .map_err(|error| error.into_api_error("task-repository"))?;
        let roots =
            platform::RegistrationRootRegistry::from_server_config(home, registration_roots)?;
        let workspaces = platform::WorkspaceRegistry::open(
            home,
            roots,
            Arc::new(RepositoryWorkspaceUsage {
                repository: repository.clone(),
            }),
            Arc::clone(&revisions),
        )?;
        let probe = Arc::new(HttpProviderProbe {
            config: Arc::clone(&config),
        });
        let bootstrap = platform::BootstrapService::open(
            home,
            Arc::clone(&config),
            Arc::clone(&workspaces),
            probe,
            Arc::clone(&revisions),
        )
        .await?;
        let settings = platform::SettingsService::open(
            home,
            Arc::clone(&config),
            Arc::clone(&workspaces),
            Arc::clone(&revisions),
            u8::try_from(limits.max_concurrent_runs).map_err(|_| {
                crate::api::ApiError::new(
                    crate::api::ApiErrorCode::InvalidRequest,
                    "max concurrent runs is invalid",
                    "settings",
                )
            })?,
        )
        .await?;
        let auth = match bearer_token {
            Some(token) => BearerTokenStore::from_token(token)?,
            None => BearerTokenStore::open(home, None)?,
        };
        let identity = bootstrap.identity();
        let platform = Arc::new(platform::PlatformServices {
            bootstrap,
            config: Arc::clone(&config),
            settings,
            workspaces: Arc::clone(&workspaces),
            auth,
            origin_policy: Arc::new(platform::OriginPolicy::new(allowed_origins)?),
            connection: crate::api::ConnectionInfo {
                server_id: identity.server_id,
                display_name: identity.display_name,
                preferred_origin: preferred_origin.clone(),
                local_url: preferred_origin,
                lan_url: lan_origin,
                plaintext: true,
            },
        });
        let factory: Arc<dyn RunDriverFactory> =
            Arc::new(run_manager::driver::KukuDriverFactory::from_platform(
                Arc::clone(&workspaces),
                Arc::clone(&config),
            ));
        let runtime = Arc::new(
            run_manager::TaskRuntime::new(
                repository,
                factory,
                workspaces,
                Arc::new(RuntimeSkillValidator),
                Arc::new(RuntimeReviewValidator),
                &limits,
            )
            .map_err(|error| error.into_api_error("task-runtime"))?,
        );
        runtime
            .recover_after_restart()
            .await
            .map_err(|error| error.into_api_error("task-recovery"))?;
        Ok(Arc::new(Self {
            platform,
            task_runtime: runtime,
            kuku_home: home.to_owned(),
            limits,
            _instance_lock: instance_lock,
        }))
    }
}

#[derive(Clone)]
struct RepositoryWorkspaceUsage {
    repository: run_manager::TaskRepository,
}

impl platform::WorkspaceUsagePort for RepositoryWorkspaceUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        id: &'a WorkspaceId,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<bool, crate::api::ApiError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.repository
                .summaries()
                .map(|summaries| summaries.iter().any(|summary| summary.workspace_id == *id))
                .map_err(|error| error.into_api_error("workspace-usage"))
        })
    }
}

struct RuntimeSkillValidator;
impl SkillSelectionValidator for RuntimeSkillValidator {
    fn validate(
        &self,
        _workspace_id: &WorkspaceId,
        tier_id: &str,
        skill_ids: &[String],
    ) -> Result<run_manager::ValidatedSkillSelection, run_manager::DomainError> {
        if tier_id.trim().is_empty() || skill_ids.iter().any(|id| id.trim().is_empty()) {
            return Err(run_manager::DomainError::InvalidRequest);
        }
        Ok(run_manager::ValidatedSkillSelection {
            selection: kuku::event::SkillsChangedFact {
                tier_id: tier_id.to_owned(),
                skill_ids: skill_ids.to_vec(),
            },
        })
    }
}

struct RuntimeReviewValidator;
impl ReviewSubmissionValidator for RuntimeReviewValidator {
    fn validate(
        &self,
        _task_id: &kuku::TaskId,
        _submission_id: &kuku::ReviewSubmissionId,
        notes: &[crate::api::ReviewAnnotationFact],
    ) -> Result<Vec<crate::api::ReviewAnnotationFact>, run_manager::DomainError> {
        Ok(notes.to_vec())
    }
}

struct HttpProviderProbe {
    config: Arc<platform::ConfigService>,
}
impl ProviderProbe for HttpProviderProbe {
    fn probe<'a>(
        &'a self,
        tier: &'a crate::api::TierSummary,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::api::TestProviderResult, crate::api::ApiError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let snapshot = self.config.snapshot().await?;
            let resolved = snapshot.resolved.ok_or_else(|| {
                crate::api::ApiError::new(
                    crate::api::ApiErrorCode::ProviderUnavailable,
                    "provider is not configured",
                    "provider-probe",
                )
            })?;
            let tier_config = resolved.tier(&tier.tier_id).ok_or_else(|| {
                crate::api::ApiError::new(
                    crate::api::ApiErrorCode::ProviderUnavailable,
                    "probe tier is not configured",
                    "provider-probe",
                )
            })?;
            let provider = resolved.provider(&tier_config.provider).ok_or_else(|| {
                crate::api::ApiError::new(
                    crate::api::ApiErrorCode::ProviderUnavailable,
                    "probe provider is not configured",
                    "provider-probe",
                )
            })?;
            let credential = provider.credential.resolve().map_err(|_| {
                crate::api::ApiError::new(
                    crate::api::ApiErrorCode::ProviderUnavailable,
                    "provider credential is unavailable",
                    "provider-probe",
                )
            })?;
            let url = format!("{}/v1/messages", provider.base_url.trim_end_matches('/'));
            let response = wreq::Client::new().post(url).header("x-api-key", credential.expose()).header("anthropic-version", "2023-06-01").json(&serde_json::json!({"model": tier_config.model, "max_tokens": 1, "messages": [{"role": "user", "content": "probe"}]})).send().await.map_err(|_| crate::api::ApiError::new(crate::api::ApiErrorCode::ProviderUnavailable, "provider probe failed", "provider-probe"))?;
            if !response.status().is_success() {
                return Err(crate::api::ApiError::new(
                    crate::api::ApiErrorCode::ProviderUnavailable,
                    "provider probe failed",
                    "provider-probe",
                ));
            }
            Ok(crate::api::TestProviderResult {
                api_version: crate::api::ApiVersion,
                reachable: true,
                provider: tier_config.provider.clone(),
                model: tier_config.model.clone(),
                message: None,
            })
        })
    }
}
