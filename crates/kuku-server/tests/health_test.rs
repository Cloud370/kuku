mod common;

#[tokio::test]
async fn health_returns_init_required_without_leaking_paths() {
    let server = common::TestServer::start_unconfigured().await;

    let resp = wreq::get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!("no-store", resp.headers()["cache-control"]);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(body["version"].as_str().is_some());
    assert_eq!(body["status"], "init_required");
    assert_eq!(body.as_object().unwrap().len(), 3);
    assert!(body.get("workspace").is_none());
    assert!(body.get("cwd").is_none());
}
