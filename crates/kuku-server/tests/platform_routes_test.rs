mod common;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::http::StatusCode;
use kuku_server::platform::{AuthContext, OriginPolicy};
use kuku_server::{advertised_origins, InterfaceAddress, ServerLimits};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let request = format!(
        "POST /api/v1/init/providers HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        server.addr,
        ServerLimits::HTTP_BODY_BYTES + 1,
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = [0_u8; 1024];
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.read(&mut response),
    )
    .await
    .expect("server did not reject oversized Content-Length")
    .unwrap();
    let response = std::str::from_utf8(&response[..read]).unwrap();
    assert!(
        response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"),
        "unexpected response: {response}"
    );
}

#[test]
fn shared_server_limits_freeze_runtime_and_transport_quotas() {
    let limits = ServerLimits::with_max_concurrent_runs(32).unwrap();

    assert_eq!(32, limits.max_concurrent_runs);
    assert_eq!(10 * 1024 * 1024, limits.http_body_bytes);
    assert_eq!(64, limits.max_queued_runs);
    assert_eq!(64, limits.max_total_streams);
    assert_eq!(8, limits.max_streams_per_task);
    assert_eq!(100, limits.max_tasks_per_page);
    assert_eq!(500, limits.max_timeline_items);
    assert_eq!(16 * 1024 * 1024, limits.max_timeline_bytes);
    assert_eq!(8, limits.review.global_scan_permits);
    assert_eq!(2, limits.review.workspace_scan_permits);
    assert_eq!(4, limits.review.global_git_permits);
    assert_eq!(1, limits.review.workspace_git_permits);
    assert!(ServerLimits::with_max_concurrent_runs(0).is_err());
    assert!(ServerLimits::with_max_concurrent_runs(65).is_err());
}

#[test]
fn wildcard_origins_publish_loopback_and_usable_lan_addresses() {
    let origins = advertised_origins(
        "0.0.0.0:17777".parse().unwrap(),
        [
            InterfaceAddress {
                name: "lo".to_owned(),
                ip: IpAddr::V4(Ipv4Addr::new(10, 255, 255, 254)),
                is_loopback: true,
            },
            InterfaceAddress {
                name: "eth0".to_owned(),
                ip: IpAddr::V4(Ipv4Addr::new(172, 20, 0, 2)),
                is_loopback: false,
            },
            InterfaceAddress {
                name: "unspecified".to_owned(),
                ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                is_loopback: false,
            },
            InterfaceAddress {
                name: "lo6".to_owned(),
                ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
                is_loopback: true,
            },
        ],
    );

    assert_eq!("http://127.0.0.1:17777", origins.local);
    assert_eq!(vec!["http://172.20.0.2:17777"], origins.lan);
    assert!(origins
        .all()
        .iter()
        .all(|origin| { !origin.contains("0.0.0.0") && !origin.contains("[::]") }));
    let policy = OriginPolicy::new(origins.all()).unwrap();
    let auth = AuthContext {
        authenticated: true,
        mode: kuku_server::api::AuthMode::Bearer,
    };
    assert!(policy
        .check_request(Some("http://172.20.0.2:17777"), &auth)
        .is_ok());
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
        ServerLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        config.path(),
        state.platform.config.snapshot().await.unwrap().path
    );
    assert_eq!(ServerLimits::default(), state.limits);
}
