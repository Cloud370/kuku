mod common;

use common::mock_provider;

#[tokio::test]
async fn health_returns_ok_and_version() {
    let mock = mock_provider::start_mock_provider();
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let resp = reqwest::get(format!("{}/health", server.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(body["version"].as_str().is_some());
}
