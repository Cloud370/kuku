mod common;

use common::mock_provider;
use common::stream::next_event_of_type;

#[tokio::test]
async fn invalid_interaction_id_returns_error() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = wreq::Client::new();
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
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = wreq::Client::new();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn response_with_wrong_run_id_does_not_resolve_permission() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "printf allowed", "timeout": 60, "brief": "print marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("toolu_cmd");
        then.status(200)
            .header("request-id", "req_final")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Allowed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    let client = wreq::Client::new();

    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "run command",
            "workspace": server.workspace.path().to_str().unwrap(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut stream = resp.bytes_stream();
    let mut stream_buf = Vec::new();
    let run_start = next_event_of_type(&mut stream, &mut stream_buf, "run_start").await;
    let permission = next_event_of_type(&mut stream, &mut stream_buf, "permission").await;

    let wrong_resp = client
        .post(format!(
            "{}/runs/wrong-{}/responses",
            server.base_url,
            run_start["run_id"].as_str().unwrap()
        ))
        .json(&serde_json::json!({
            "interaction_id": permission["id"].as_str().unwrap(),
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(wrong_resp.status(), 200);
    let body: serde_json::Value = wrong_resp.json().await.unwrap();
    assert_eq!(body["ok"], false);

    let correct_resp = client
        .post(format!(
            "{}/runs/{}/responses",
            server.base_url,
            run_start["run_id"].as_str().unwrap()
        ))
        .json(&serde_json::json!({
            "interaction_id": permission["id"].as_str().unwrap(),
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(correct_resp.status(), 200);
    let body: serde_json::Value = correct_resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_response_preserves_conversation_identity() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_tool_review")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_tool_review",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_review", "name": "run_command", "input": {"command": "printf review", "timeout": 60, "brief": "print review marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("toolu_review");
        then.status(200)
            .header("request-id", "req_final_review")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_final_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Allowed in review."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    let client = wreq::Client::new();

    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "review command",
            "workspace": server.workspace.path().to_str().unwrap(),
            "conversation": "review",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut stream = resp.bytes_stream();
    let mut stream_buf = Vec::new();
    let run_start = next_event_of_type(&mut stream, &mut stream_buf, "run_start").await;
    let permission = next_event_of_type(&mut stream, &mut stream_buf, "permission").await;
    assert_eq!(permission["conversation"], "review");

    let correct_resp = client
        .post(format!(
            "{}/runs/{}/responses",
            server.base_url,
            run_start["run_id"].as_str().unwrap()
        ))
        .json(&serde_json::json!({
            "interaction_id": permission["id"].as_str().unwrap(),
            "conversation": "review",
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(correct_resp.status(), 200);
    let body: serde_json::Value = correct_resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
}
