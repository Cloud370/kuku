//! Embedded WebUI static file serving for the kuku unified binary.
//!
//! Embeds `apps/web/dist/` at compile time via `rust-embed` and adds an
//! SPA fallback so the React router handles client-side paths.

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::Response,
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

/// Start the HTTP server, optionally with embedded WebUI (SPA fallback).
pub async fn run_server(
    args: kuku_server::server_args::ServerArgs,
    web_ui: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if web_ui && !cfg!(feature = "embedded-web-assets") {
        return Err(
            "Web UI assets are not bundled. Build apps/web and rebuild with --features embedded-web-assets."
                .into(),
        );
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let prepared = kuku_server::prepare_server(args).await?;
    prepared.print_connection_info(web_ui);
    if web_ui {
        println!("warning: LAN plaintext connections are visible to the local network; use external TLS for untrusted networks");
    }

    let app = if web_ui {
        prepared.app.clone().fallback(serve_spa)
    } else {
        prepared.app.clone()
    };
    prepared.serve_with(app).await?;

    Ok(())
}
