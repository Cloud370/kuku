pub mod api;
pub mod config_watcher;
pub mod error_mapping;
pub mod platform;
pub mod routes;
pub mod run_manager;
pub mod server_args;
#[cfg(feature = "test-scenarios")]
pub mod testing;
pub mod wire;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use kuku::event::WorkspaceId;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use platform::{AuthPolicy, BearerTokenStore, ProviderProbe};
use run_manager::driver::RunDriverFactory;
use run_manager::{ReviewSubmissionValidator, SkillSelectionValidator};

pub struct AppState {
    pub platform: Arc<platform::PlatformServices>,
    pub task_runtime: Arc<run_manager::TaskRuntime>,
    pub kuku_home: PathBuf,
    _instance_lock: platform::ServerInstanceLock,
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
    secure_response(&state, next.run(request).await)
}

pub fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", axum::routing::get(routes::health::health))
        .nest(
            "/api/v1",
            platform::router(state.platform.clone()).merge(routes::tasks::router(
                state.task_runtime.clone(),
                state.platform.bootstrap.clone(),
            )),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024))
}

pub async fn start_server(
    config: kuku::config::Config,
    password: Option<String>,
    max_concurrent_runs: usize,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let kuku_home = kuku::session::kuku_home().unwrap_or_else(|_| PathBuf::from(".kuku"));
    let state = AppState::open(
        &kuku_home,
        Some(config),
        password,
        vec![platform::RegistrationRootSpec {
            label: "Current directory".to_owned(),
            path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }],
        "http://127.0.0.1:17777".to_owned(),
        max_concurrent_runs,
    )
    .await
    .expect("server services must initialize");

    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    (addr, handle)
}

pub async fn shutdown_signal(state: Arc<AppState>) {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");

    let _ = state;
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
    pub async fn open(
        home: &std::path::Path,
        _initial_config: Option<kuku::config::Config>,
        bearer_token: Option<String>,
        registration_roots: Vec<platform::RegistrationRootSpec>,
        preferred_origin: String,
        max_concurrent_runs: usize,
    ) -> Result<Arc<Self>, crate::api::ApiError> {
        let instance_lock = platform::ServerInstanceLock::acquire(home).map_err(|error| {
            crate::api::ApiError::new(
                crate::api::ApiErrorCode::Internal,
                error.to_string(),
                "server-lock",
            )
        })?;
        let revisions = platform::ServerRevisionCoordinator::open(home);
        let config =
            platform::ConfigService::open(home.join("config.toml"), Arc::clone(&revisions)).await?;
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
            u8::try_from(max_concurrent_runs).map_err(|_| {
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
            origin_policy: Arc::new(platform::OriginPolicy::new(vec![preferred_origin.clone()])?),
            connection: crate::api::ConnectionInfo {
                server_id: identity.server_id,
                display_name: identity.display_name,
                preferred_origin: preferred_origin.clone(),
                local_url: preferred_origin,
                lan_url: None,
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
                max_concurrent_runs,
                64,
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
