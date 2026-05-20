mod config_watcher;
mod error_mapping;
mod routes;
mod run_manager;
mod wire;

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use clap::Parser;
use serde_json::json;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use run_manager::RunManager;

pub struct AppState {
    pub run_manager: Mutex<RunManager>,
    pub config: Arc<ArcSwap<kuku::config::Config>>,
    pub password: Option<String>,
}

#[derive(Parser)]
#[command(name = "kuku-server", about = "HTTP API host for kuku SDK")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:17777")]
    listen: String,

    #[arg(long)]
    config: Option<String>,

    #[arg(long)]
    password: Option<String>,

    #[arg(long, default_value = "16")]
    max_concurrent_runs: usize,
}

fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

async fn auth_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    if is_loopback(&addr) {
        return next.run(request).await;
    }

    if let Some(ref expected) = state.password {
        if let Some(auth_header) = request.headers().get(header::AUTHORIZATION) {
            if let Ok(auth_str) = auth_header.to_str() {
                if auth_str == format!("Bearer {expected}") {
                    return next.run(request).await;
                }
            }
        }
    }

    (
        StatusCode::OK,
        Json(json!({"ok": false, "code": "auth_required", "message": "password required"})),
    )
        .into_response()
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let listen_addr: SocketAddr = match args.listen.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: invalid listen address: {e}");
            std::process::exit(1);
        }
    };

    if !listen_addr.ip().is_loopback() && args.password.is_none() {
        eprintln!("error: --password is required for non-loopback addresses");
        std::process::exit(1);
    }

    let config_path = args
        .config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                .join(".kuku")
                .join("config.toml")
        });

    if !config_path.exists() {
        eprintln!("error: config file not found: {}", config_path.display());
        std::process::exit(1);
    }

    let config = match kuku::config::load_config(&config_path).and_then(|f| f.resolve()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to load config: {e}");
            std::process::exit(1);
        }
    };

    let config_store: Arc<ArcSwap<kuku::config::Config>> =
        Arc::new(ArcSwap::from_pointee(config));
    let _watcher = config_watcher::ConfigWatcher::start(config_path.clone(), config_store.clone());

    let state = Arc::new(AppState {
        run_manager: Mutex::new(RunManager::new(args.max_concurrent_runs)),
        config: config_store,
        password: args.password,
    });

    let app = Router::new()
        .route("/health", get(routes::health::health))
        .route("/sessions/{id}/events", get(routes::events::events))
        .route("/runs", post(routes::runs::create_run))
        .route("/runs/{id}", delete(routes::runs::cancel_run))
        .route("/runs/{id}/responses", post(routes::responses::respond))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: failed to bind {listen_addr}: {e}");
            std::process::exit(1);
        });

    tracing::info!("listening on {listen_addr}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(state.clone()))
    .await
    .unwrap();
}

async fn shutdown_signal(state: Arc<AppState>) {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");

    let run_ids = {
        let mgr = state.run_manager.lock().await;
        mgr.active_run_ids()
    };

    for run_id in run_ids {
        let mut mgr = state.run_manager.lock().await;
        mgr.cancel(&run_id);
    }
}
