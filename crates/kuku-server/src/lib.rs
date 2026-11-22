//! HTTP access to the kuku Session runtime.

pub mod config_watcher;
pub mod error_mapping;
pub mod routes;
pub mod run_manager;
pub mod server_args;
pub mod service;
pub mod wire;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use tokio::sync::Mutex;

use crate::run_manager::RunManager;

/// Build version shared by the server and packaged clients.
pub use kuku::BUILD_VERSION;

/// Shared host resources and authorization supplied to HTTP handlers.
pub struct AppState {
    /// Executor responsible for active and recently observed Sessions.
    pub run_manager: Mutex<RunManager>,
    /// Configuration read and resolved on the server.
    pub config: Arc<ArcSwap<kuku::config::Config>>,
    /// Token required by Session routes.
    pub password: Option<String>,
    /// User data directory containing the Sessions.
    pub kuku_home: PathBuf,
    /// Authority consulted before exact tool actions are dispatched.
    pub authorizer: Arc<dyn kuku::permission::ToolAuthorizer>,
}

/// Build the authenticated HTTP API around an embedding host's resources.
pub fn build_app(state: Arc<AppState>) -> Router {
    service::app(state)
}

/// Start an embedded HTTP service on a free loopback port.
pub async fn start_server(
    config: kuku::config::Config,
    password: Option<String>,
    max_concurrent_runs: usize,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let kuku_home = kuku::session::kuku_home().unwrap_or_else(|_| PathBuf::from(".kuku"));
    let config_store = Arc::new(ArcSwap::from_pointee(config));

    let state = Arc::new(AppState {
        run_manager: Mutex::new(RunManager::new(max_concurrent_runs)),
        config: config_store,
        password,
        kuku_home,
        authorizer: Arc::new(kuku::permission::TrustedLocal),
    });

    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (addr, handle)
}
