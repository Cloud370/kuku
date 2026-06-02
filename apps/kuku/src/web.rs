//! Embedded WebUI static file serving for the kuku unified binary.
//!
//! Embeds `apps/web/dist/` at compile time via `rust-embed` and adds an
//! SPA fallback so the React router handles client-side paths.

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::Response,
    Router,
};

#[cfg(feature = "embedded-web-assets")]
use rust_embed::RustEmbed;

#[cfg(feature = "embedded-web-assets")]
#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct WebAssets;

#[cfg(feature = "embedded-web-assets")]
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    }
}

#[cfg(feature = "embedded-web-assets")]
async fn serve_spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = WebAssets::get(path) {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(path))
            .body(Body::from(file.data))
            .unwrap();
    }

    if let Some(index) = WebAssets::get("index.html") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type("index.html"))
            .body(Body::from(index.data))
            .unwrap();
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not found"))
        .unwrap()
}

#[cfg(not(feature = "embedded-web-assets"))]
async fn serve_spa(_uri: Uri) -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(
            "Web UI assets are not bundled. Build apps/web and rebuild with --features embedded-web-assets.",
        ))
        .unwrap()
}

async fn setup_server(
    args: kuku_server::server_args::ServerArgs,
) -> Result<
    (
        Router,
        tokio::net::TcpListener,
        std::sync::Arc<kuku_server::AppState>,
    ),
    Box<dyn std::error::Error>,
> {
    use kuku_server::run_manager::RunManager;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let listen_addr: SocketAddr = args.listen.parse()?;

    if !listen_addr.ip().is_loopback() && args.password.is_none() {
        return Err("--password is required for non-loopback addresses".into());
    }

    let config_path = args
        .config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            home::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                .join(".kuku")
                .join("config.toml")
        });

    if !config_path.exists() {
        return Err(format!("config file not found: {}", config_path.display()).into());
    }

    let config = kuku::config::load_and_patch_config(&config_path).and_then(|f| f.resolve())?;

    let config_store = Arc::new(arc_swap::ArcSwap::from_pointee(config));
    let _watcher =
        kuku_server::config_watcher::ConfigWatcher::start(config_path, config_store.clone());

    let state = Arc::new(kuku_server::AppState {
        run_manager: Mutex::new(RunManager::new(args.max_concurrent_runs)),
        config: config_store,
        password: args.password,
    });

    let app = kuku_server::build_app(state.clone());
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;

    Ok((app, listener, state))
}

/// Start the HTTP server, optionally with embedded WebUI (SPA fallback).
pub async fn run_server(
    args: kuku_server::server_args::ServerArgs,
    web_ui: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::net::SocketAddr;

    if web_ui && !cfg!(feature = "embedded-web-assets") {
        return Err(
            "Web UI assets are not bundled. Build apps/web and rebuild with --features embedded-web-assets."
                .into(),
        );
    }

    let (app, listener, state) = setup_server(args).await?;

    let app = if web_ui { app.fallback(serve_spa) } else { app };

    let label = if web_ui { " (web UI enabled)" } else { "" };
    tracing::info!("listening on {}{label}", listener.local_addr()?);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(kuku_server::shutdown_signal(state.clone()))
    .await?;

    Ok(())
}
