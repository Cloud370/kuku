mod common;

use common::mock_provider;
use common::stream::{next_event_of_type, next_terminal_event};

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

    let log = lines
        .iter()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|value| value["type"] == "log" && value["record"]["kind"] == "runtime.model_request")
        .expect("expected runtime log record in server stream");
    assert_eq!(log["record"]["scope"], "runtime");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_while_waiting_for_permission_stops_run() {
    let mock = mock_provider::start_mock_provider().await;
    let final_mock = mock.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("permission gate denied this tool call")
            .body_contains(r#""tool_result""#)
            .body_contains("toolu_cancel_wait");
        then.status(200)
            .header("request-id", "req_after_cancel")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_after_cancel",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Should not continue after cancel."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("cancel permission wait");
        then.status(200)
            .header("request-id", "req_tool")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_cancel_wait", "name": "run_command", "input": {"command": "printf should-not-run", "timeout": 60, "brief": "print cancelled marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

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
            "prompt": "cancel permission wait",
            "workspace": server.workspace.path().to_str().unwrap(),
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let mut stream = resp.bytes_stream();
    let mut stream_buf = Vec::new();
    let run_start = next_event_of_type(&mut stream, &mut stream_buf, "run_start").await;
    let _permission = next_event_of_type(&mut stream, &mut stream_buf, "permission").await;

    let cancel_resp = client
        .delete(format!(
            "{}/runs/{}",
            server.base_url,
            run_start["run_id"].as_str().unwrap()
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(cancel_resp.status(), 200);
    let body: serde_json::Value = cancel_resp.json().await.unwrap();
    assert_eq!(body["ok"], true);

    let terminal = next_terminal_event(&mut stream, &mut stream_buf).await;
    assert_eq!(terminal["type"], "cancelled");
    final_mock.assert_hits(0);
}
