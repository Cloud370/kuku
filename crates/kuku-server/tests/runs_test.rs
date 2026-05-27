mod common;

use common::mock_provider;

async fn wait_for_server(base_url: &str) {
    let client = wreq::Client::new();
    for _ in 0..50 {
        if let Ok(resp) = client.get(format!("{base_url}/health")).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("server did not become ready at {base_url}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_run_lifecycle() {
    let mock = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&mock, "Hello from server!");

    let warmup = wreq::Client::new();
    let _ = warmup
        .post(format!("http://127.0.0.1:{}/v1/messages", mock.port()))
        .send()
        .await;

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    wait_for_server(&server.base_url).await;

    let client = wreq::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "hello",
            "workspace": server.workspace.path().to_str().unwrap(),
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body = resp.text().await.unwrap();
    assert_eq!(
        content_type, "application/x-ndjson",
        "unexpected content-type, body: {body}"
    );

    let lines: Vec<&str> = body.trim().split('\n').collect();
    assert!(
        lines.len() >= 2,
        "expected at least run_start and done events"
    );

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["type"], "run_start");
    assert!(first["run_id"].as_str().is_some());

    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["type"], "done");
}

#[tokio::test]
async fn cancel_nonexistent_run_returns_not_found() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    wait_for_server(&server.base_url).await;

    let client = wreq::Client::new();
    let resp = client
        .delete(format!("{}/runs/nonexistent", server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["code"], "session_not_found");
}

#[tokio::test]
async fn invalid_workspace_returns_error() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    wait_for_server(&server.base_url).await;

    let client = wreq::Client::new();
    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "hello",
            "workspace": "/nonexistent/path/that/does/not/exist",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["code"], "invalid_request");
}
