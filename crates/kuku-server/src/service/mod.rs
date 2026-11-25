//! HTTP hosting, local service discovery and orderly process handoff.

mod access;
mod discovery;
mod model_default;

use std::error::Error;
use std::fs;
use std::future::IntoFuture;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Serialize;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock, watch};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::config_watcher::{ConfigSource, ConfigWatcher};
use crate::routes;
use crate::run_manager::RunManager;
use crate::server_args::ServerArgs;
use crate::{AppState, BUILD_VERSION};

use self::access::{Access, BrowserAccess};
use self::discovery::{Discovery, Lease};

/// Protocol revision for Session command and observation routes.
pub const SESSION_PROTOCOL_VERSION: u32 = 2;

/// Independent protocol revision for status, restart and browser handoff.
pub const MANAGEMENT_VERSION: u32 = 1;

const MAX_REQUEST_BYTES: usize = 10 * 1024 * 1024;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

struct Identity {
    instance_id: String,
    pid: u32,
    host: String,
    origin_header: HeaderValue,
}

impl Identity {
    fn new(record: &Discovery) -> Self {
        let host = record.address.to_string();
        let origin_header = HeaderValue::from_str(&format!("http://{host}"))
            .expect("a socket address is always valid in an Origin header");
        Self {
            instance_id: record.instance_id.clone(),
            pid: record.pid,
            host,
            origin_header,
        }
    }
}

struct ServiceState {
    app: Arc<AppState>,
    identity: Option<Identity>,
    config_source: Option<Arc<ConfigSource>>,
    browser: Option<BrowserAccess>,
    admission: RwLock<()>,
    draining: AtomicBool,
    stop: watch::Sender<bool>,
}

impl ServiceState {
    fn new(
        app: Arc<AppState>,
        identity: Option<Identity>,
        config_source: Option<Arc<ConfigSource>>,
        browser: Option<BrowserAccess>,
    ) -> Self {
        Self {
            app,
            identity,
            config_source,
            browser,
            admission: RwLock::new(()),
            draining: AtomicBool::new(false),
            stop: watch::channel(false).0,
        }
    }

    async fn wait_for_stop(&self) {
        let _ = self.stop.subscribe().wait_for(|stopping| *stopping).await;
    }

    async fn run_manager(&self) -> RunManager {
        self.app.run_manager.lock().await.clone()
    }

    async fn close_on_signal(&self) {
        let _admission = self.admission.write().await;
        if !self.draining.swap(true, Ordering::AcqRel) {
            if let Err(error) = self.run_manager().await.close_all().await {
                tracing::error!(%error, "session cleanup failed");
            }
        }
        self.stop.send_replace(true);
    }
}

#[derive(Serialize)]
struct ServiceStatus {
    version: &'static str,
    protocol_version: u32,
    management_version: u32,
    instance_id: Option<String>,
    pid: Option<u32>,
    data_dir: String,
    config_path: Option<String>,
    config_sha256: Option<String>,
    config_default_model: Option<kuku::config::ModelRef>,
    busy: Vec<String>,
    draining: bool,
}

/// Run the selected HTTP service until shutdown or orderly replacement.
pub async fn serve(args: ServerArgs, web: Option<Router>) -> Result<(), Box<dyn Error>> {
    serve_with_cli(args, web, None).await
}

/// Serve from the installed CLI's managed entrypoint.
pub async fn serve_managed(
    mut args: ServerArgs,
    web: Option<Router>,
) -> Result<(), Box<dyn Error>> {
    args.managed = true;
    serve_with_cli(args, web, std::env::current_exe().ok()).await
}

async fn serve_with_cli(
    args: ServerArgs,
    web: Option<Router>,
    managed_cli: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(io::stderr)
        .try_init();
    let listen: SocketAddr = args.listen.parse()?;
    let password = if args.managed {
        if !listen.ip().is_loopback() || args.password.is_some() {
            return Err("a managed server uses a loopback address and private access token".into());
        }
        None
    } else {
        Some(explicit_password(args.password)?)
    };

    let data_dir = args.data_dir.map_or_else(kuku::session::kuku_home, Ok)?;
    fs::create_dir_all(&data_dir)?;
    let data_dir = fs::canonicalize(data_dir)?;
    let lease = args
        .managed
        .then(|| Lease::acquire(&data_dir))
        .transpose()?;
    let config_path = args.config.unwrap_or_else(|| data_dir.join("config.toml"));
    let config_path = canonical_config(config_path)?;
    let (config_source, config) = ConfigSource::open(config_path)?;
    let config_store = Arc::new(ArcSwap::from_pointee(config));
    let _watcher = ConfigWatcher::start(config_source.clone(), config_store.clone());

    let listener = bind(listen, args.managed).await?;
    let addr = listener.local_addr()?;
    let record = args
        .managed
        .then(|| Discovery::new(addr, &data_dir, &config_source.path))
        .transpose()?;
    let password = record
        .as_ref()
        .map(|found| found.token.clone())
        .or(password);
    let browser = (args.managed && web.is_some())
        .then(BrowserAccess::new)
        .transpose()?;
    let state = Arc::new(AppState {
        run_manager: Mutex::new(RunManager::new(args.max_concurrent_runs)),
        config: config_store,
        password,
        kuku_home: data_dir,
        managed_cli,
        authorizer: Arc::new(kuku::permission::TrustedLocal),
    });
    let service = Arc::new(ServiceState::new(
        state,
        record.as_ref().map(Identity::new),
        Some(config_source),
        browser,
    ));
    let app = router(service.clone(), web);
    if let (Some(lease), Some(record)) = (&lease, &record) {
        lease.publish(record)?;
    }
    tracing::info!("listening on {addr}");
    run(listener, app, service).await?;
    Ok(())
}

pub(crate) fn app(state: Arc<AppState>) -> Router {
    router(Arc::new(ServiceState::new(state, None, None, None)), None)
}

fn router(service: Arc<ServiceState>, web: Option<Router>) -> Router {
    let sessions = routes::interactive()
        .with_state(service.app.clone())
        .layer(Extension(service.stop.subscribe()))
        .layer(middleware::from_fn_with_state(service.clone(), admit));
    let protected = Router::new()
        .route("/service/status", get(status))
        .route("/service/model-default-plan", get(model_default::plan))
        .route("/service/restart", post(restart))
        .route("/service/browser-ticket", post(access::issue))
        .with_state(service.clone())
        .merge(sessions)
        .layer(middleware::from_fn_with_state(
            service.clone(),
            access::authorize,
        ));
    let app = Router::new()
        .route("/health", get(routes::health::health))
        .route("/service/browser-session", post(access::exchange))
        .with_state(service.clone())
        .merge(protected);
    let app = match web {
        Some(web) => app.merge(web),
        None => app,
    };
    app.layer(middleware::from_fn_with_state(service, access::origin))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(MAX_REQUEST_BYTES))
}

async fn admit(State(service): State<Arc<ServiceState>>, request: Request, next: Next) -> Response {
    if matches!(
        request.method(),
        &Method::GET | &Method::HEAD | &Method::OPTIONS
    ) {
        return next.run(request).await;
    }
    let _admission = service.admission.read().await;
    if service.draining.load(Ordering::Acquire) {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_draining",
            "the server is releasing Sessions before restart",
        );
    }
    next.run(request).await
}

async fn status(State(service): State<Arc<ServiceState>>) -> Response {
    let loaded = service
        .config_source
        .as_ref()
        .map(|source| source.loaded.load_full());
    let status = ServiceStatus {
        version: BUILD_VERSION,
        protocol_version: SESSION_PROTOCOL_VERSION,
        management_version: MANAGEMENT_VERSION,
        instance_id: service
            .identity
            .as_ref()
            .map(|identity| identity.instance_id.clone()),
        pid: service.identity.as_ref().map(|identity| identity.pid),
        data_dir: service.app.kuku_home.to_string_lossy().into_owned(),
        config_path: service
            .config_source
            .as_ref()
            .map(|source| source.path.to_string_lossy().into_owned()),
        config_sha256: loaded.as_ref().map(|config| config.sha256.clone()),
        config_default_model: loaded.map(|config| config.default_model.clone()),
        busy: service.run_manager().await.active_run_ids(),
        draining: service.draining.load(Ordering::Acquire),
    };
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(status),
    )
        .into_response()
}

async fn restart(
    State(service): State<Arc<ServiceState>>,
    Extension(access): Extension<Access>,
) -> Response {
    if access != Access::Native {
        return access::native_access_required();
    }
    if service.identity.is_none() {
        return failure(
            StatusCode::CONFLICT,
            "managed_service_required",
            "restart is available for the managed local server",
        );
    }
    match tokio::spawn(close_for_restart(service)).await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(%error, "managed restart task failed");
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "service_cleanup_failed",
                "the server could not release its Sessions for restart",
            )
        }
    }
}

async fn close_for_restart(service: Arc<ServiceState>) -> Response {
    let _admission = match service.admission.try_write() {
        Ok(guard) => guard,
        Err(_)
            if service.admission.try_read().is_ok()
                && !service.draining.load(Ordering::Acquire) =>
        {
            return restart_busy(service.run_manager().await.active_run_ids());
        }
        // NOTE: Wait for a concurrent service shutdown to finish releasing its writers.
        Err(_) => service.admission.write().await,
    };
    if service.draining.load(Ordering::Acquire) {
        return StatusCode::ACCEPTED.into_response();
    }
    let manager = service.run_manager().await;
    let busy = manager.active_run_ids();
    if !busy.is_empty() {
        return restart_busy(busy);
    }
    service.draining.store(true, Ordering::Release);
    if let Err(error) = manager.close_all().await {
        service.draining.store(false, Ordering::Release);
        tracing::error!(%error, "managed server could not release Sessions");
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "service_cleanup_failed",
            "the server could not release its Sessions for restart",
        );
    }
    service.stop.send_replace(true);
    StatusCode::ACCEPTED.into_response()
}

fn restart_busy(mut busy: Vec<String>) -> Response {
    if busy.is_empty() {
        busy.push("Session command in progress".into());
    }
    (
        StatusCode::CONFLICT,
        Json(json!({
            "ok": false,
            "code": "service_busy",
            "message": "restart waits for Session commands, active execution, and tool cleanup",
            "busy": busy
        })),
    )
        .into_response()
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response = (
        status,
        Json(json!({ "ok": false, "code": code, "message": message })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn explicit_password(argument: Option<String>) -> Result<String, Box<dyn Error>> {
    let password = argument
        .or_else(|| std::env::var("KUKU_SERVER_PASSWORD").ok())
        .filter(|candidate| !candidate.trim().is_empty())
        .ok_or("--password or KUKU_SERVER_PASSWORD is required for a standalone server")?;
    if HeaderValue::from_str(&password).is_err() || !password.is_ascii() {
        return Err("the server access token must be visible ASCII".into());
    }
    Ok(password)
}

fn canonical_config(path: PathBuf) -> io::Result<PathBuf> {
    fs::canonicalize(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot resolve config file {}: {error}", path.display()),
        )
    })
}

async fn bind(listen: SocketAddr, managed: bool) -> io::Result<TcpListener> {
    match TcpListener::bind(listen).await {
        Err(error) if managed && error.kind() == io::ErrorKind::AddrInUse => {
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await
        }
        result => result,
    }
}

async fn run(listener: TcpListener, app: Router, service: Arc<ServiceState>) -> io::Result<()> {
    let serve = axum::serve(listener, app)
        .with_graceful_shutdown({
            let service = service.clone();
            async move { service.wait_for_stop().await }
        })
        .into_future();
    tokio::pin!(serve);
    tokio::select! {
        result = &mut serve => return result,
        () = service.wait_for_stop() => {}
        () = wait_for_signal() => service.close_on_signal().await,
    }
    match tokio::time::timeout(SHUTDOWN_TIMEOUT, serve).await {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                () = wait_for_control_c() => {},
                _ = terminate.recv() => {},
            }
            return;
        }
    }
    wait_for_control_c().await;
}

async fn wait_for_control_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::debug!(%error, "console shutdown signal is unavailable");
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use std::future::{Future, poll_fn};
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::Poll;

    use arc_swap::ArcSwap;
    use kuku::config::{Config, ModelRef};
    use tokio::sync::Mutex;

    use super::{ServiceState, close_for_restart};
    use crate::AppState;
    use crate::run_manager::RunManager;

    #[tokio::test]
    async fn restart_defers_while_a_session_command_holds_admission() {
        let config = Config {
            models: Default::default(),
            providers: Default::default(),
            default_model: ModelRef::new("unused", "unused"),
            discovery: Default::default(),
            handoff: Default::default(),
            logs: Default::default(),
            plugin: Default::default(),
            update: Default::default(),
        };
        let app = Arc::new(AppState {
            run_manager: Mutex::new(RunManager::new(1)),
            config: Arc::new(ArcSwap::from_pointee(config)),
            password: None,
            kuku_home: Default::default(),
            managed_cli: None,
            authorizer: Arc::new(kuku::permission::TrustedLocal),
        });
        let service = Arc::new(ServiceState::new(app, None, None, None));
        let command = service.admission.read().await;
        let mut restart = pin!(close_for_restart(service.clone()));

        let Poll::Ready(response) =
            poll_fn(|context| Poll::Ready(restart.as_mut().poll(context))).await
        else {
            panic!("restart must report the active Session command instead of waiting for it");
        };
        assert_eq!(409, response.status());

        drop(command);
        assert_eq!(202, close_for_restart(service).await.status());
    }
}
