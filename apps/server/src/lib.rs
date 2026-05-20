pub mod config_watcher;
pub mod error_mapping;
pub mod routes;
pub mod run_manager;
pub mod wire;

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
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

pub fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
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
        .with_state(state)
}

pub async fn start_server(
    config: kuku::config::Config,
    password: Option<String>,
    max_concurrent_runs: usize,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let config_store = Arc::new(ArcSwap::from_pointee(config));

    let state = Arc::new(AppState {
        run_manager: Mutex::new(RunManager::new(max_concurrent_runs)),
        config: config_store,
        password,
    });

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
