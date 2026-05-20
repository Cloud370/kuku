mod common;

use common::mock_provider;

#[tokio::test]
async fn nonexistent_session_returns_not_found() {
    let mock = mock_provider::start_mock_provider();
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "{}/sessions/nonexistent/events",
            server.base_url
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["code"], "session_not_found");
}

#[tokio::test]
async fn health_passes_even_with_password_set() {
    let mock = mock_provider::start_mock_provider();
    let config = mock_provider::make_test_config(mock.port());
    let server =
        common::TestServer::start_with_password(config, Some("testpass".to_string())).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
}
