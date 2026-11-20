use axum::Router;

#[cfg(feature = "embedded-web-assets")]
use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
#[cfg(feature = "embedded-web-assets")]
use rust_embed::RustEmbed;

#[cfg(feature = "embedded-web-assets")]
#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct WebAssets;

#[cfg(feature = "embedded-web-assets")]
const CONTENT_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'; connect-src 'self'; font-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

#[cfg(feature = "embedded-web-assets")]
fn static_file(path: &str, content_type: &'static str, cache: &'static str) -> Response {
    let Some(file) = WebAssets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .body(Body::from(file.data))
        .expect("static response headers are valid")
}

#[cfg(feature = "embedded-web-assets")]
async fn index() -> Response {
    let mut response = static_file("index.html", "text/html; charset=utf-8", "no-store");
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static(CONTENT_POLICY),
    );
    response
}

#[cfg(feature = "embedded-web-assets")]
async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let content_type = match path.rsplit('.').next().unwrap_or_default() {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    };
    static_file(path, content_type, "no-cache")
}

#[cfg(feature = "embedded-web-assets")]
pub(crate) fn assets() -> Option<Router> {
    Some(
        Router::new()
            .route("/", get(index))
            .route("/session/{id}", get(index))
            .route("/assets/{*path}", get(asset))
            .route("/favicon.svg", get(asset)),
    )
}

#[cfg(not(feature = "embedded-web-assets"))]
pub(crate) fn assets() -> Option<Router> {
    None
}
