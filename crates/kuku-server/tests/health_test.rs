mod common;

use common::mock_provider;
use kuku_server::service::{MANAGEMENT_VERSION, SESSION_PROTOCOL_VERSION};

#[tokio::test]
async fn health_returns_ok_and_version() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let resp = wreq::get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(true, body["ok"]);
    assert_eq!(kuku_server::BUILD_VERSION, body["version"]);
    assert_eq!(SESSION_PROTOCOL_VERSION, body["protocol_version"]);
    assert_eq!(MANAGEMENT_VERSION, body["management_version"]);
    assert!(body.get("workspace").is_none());
}

#[tokio::test]
async fn loopback_status_and_session_access_require_a_token() {
    let config = mock_provider::make_test_config(9);
    let server = common::TestServer::start(config).await;
    let http = wreq::Client::builder().no_proxy().build().unwrap();

    for path in ["/service/status", "/sessions"] {
        let response = http
            .get(format!("{}{path}", server.base_url))
            .header("Origin", "https://example.test")
            .send()
            .await
            .unwrap();
        assert_eq!(401, response.status());
        assert_eq!("Bearer", response.headers()["www-authenticate"]);
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!("auth_required", body["code"]);
    }

    let response = server
        .request(wreq::Method::GET, "/service/status")
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    let status: serde_json::Value = response.json().await.unwrap();
    assert_eq!(MANAGEMENT_VERSION, status["management_version"]);
    assert_eq!(kuku_server::BUILD_VERSION, status["version"]);
}
