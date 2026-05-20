mod common;

use common::mock_provider;

#[tokio::test]
async fn invalid_interaction_id_returns_error() {
    let mock = mock_provider::start_mock_provider();
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/runs/fake_run/responses", server.base_url))
        .json(&serde_json::json!({
            "interaction_id": "nonexistent",
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn invalid_choice_returns_error() {
    let mock = mock_provider::start_mock_provider();
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/runs/fake_run/responses", server.base_url))
        .json(&serde_json::json!({
            "interaction_id": "any",
            "choice": "invalid_choice",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["code"], "invalid_request");
}
