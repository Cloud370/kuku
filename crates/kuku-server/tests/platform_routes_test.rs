mod common;

use axum::http::StatusCode;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn bearer(request: wreq::RequestBuilder) -> wreq::RequestBuilder {
    request.header("authorization", format!("Bearer {TOKEN}"))
}

#[tokio::test]
async fn init_gate_allows_status_init_and_registration_roots_only() {
    let server = common::TestServer::start_unconfigured().await;
    let client = wreq::Client::new();

    let status = bearer(client.get(format!("{}/api/v1/status", server.base_url)))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, status.status());
    assert_eq!("no-store", status.headers()["cache-control"]);
    assert_eq!(
        "required",
        status.json::<serde_json::Value>().await.unwrap()["init"]["phase"]
    );

    let init = bearer(client.get(format!("{}/api/v1/init/status", server.base_url)))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, init.status());

    let roots = bearer(client.get(format!("{}/api/v1/registration-roots", server.base_url)))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, roots.status());
    let roots_body: serde_json::Value = roots.json().await.unwrap();
    assert!(roots_body.to_string().contains("root_"));
    assert!(!roots_body.to_string().contains("\"path\""));

    let settings = bearer(client.get(format!("{}/api/v1/settings", server.base_url)))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::CONFLICT, settings.status());
    assert_eq!(
        "init_incomplete",
        settings.json::<serde_json::Value>().await.unwrap()["code"]
    );

    let tasks = bearer(client.get(format!("{}/api/v1/tasks", server.base_url)))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::CONFLICT, tasks.status());
    assert_eq!(
        "init_incomplete",
        tasks.json::<serde_json::Value>().await.unwrap()["code"]
    );
}

#[tokio::test]
async fn legacy_routes_are_gone_and_body_limit_is_shared() {
    let server = common::TestServer::start_unconfigured().await;
    let client = wreq::Client::new();
    for path in [
        "/sessions",
        "/runs",
        "/responses",
        "/events",
        "/api/v1/auth/verify",
    ] {
        let response = bearer(client.get(format!("{}{}", server.base_url, path)))
            .send()
            .await
            .unwrap();
        assert_eq!(StatusCode::NOT_FOUND, response.status(), "{path}");
    }

    let oversized = "x".repeat(10 * 1024 * 1024 + 1);
    let response = bearer(
        client
            .post(format!("{}/api/v1/init/providers", server.base_url))
            .header("content-type", "application/json")
            .body(oversized),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(StatusCode::PAYLOAD_TOO_LARGE, response.status());
}

#[tokio::test]
async fn explicit_config_path_is_retained_by_prepared_services() {
    let home = tempfile::tempdir().unwrap();
    let config = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(config.path(), "").unwrap();
    let state = kuku_server::AppState::open_with_config_path(
        home.path(),
        config.path(),
        Some(TOKEN.to_owned()),
        Vec::new(),
        "http://127.0.0.1".to_owned(),
        16,
    )
    .await
    .unwrap();
    assert_eq!(
        config.path(),
        state.platform.config.snapshot().await.unwrap().path
    );
}
