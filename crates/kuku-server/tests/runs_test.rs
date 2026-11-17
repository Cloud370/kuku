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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_start_uses_server_scoped_home_after_other_server_drops() {
    let mock = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&mock, "Hello from isolated home!");

    let config = mock_provider::make_test_config(mock.port());
    let server_a = common::TestServer::start(config.clone()).await;
    let server_b = common::TestServer::start(config).await;
    wait_for_server(&server_b.base_url).await;
    drop(server_a);

    let client = wreq::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/runs", server_b.base_url))
        .json(&serde_json::json!({
            "prompt": "hello after drop",
            "workspace": server_b.workspace.path().to_str().unwrap(),
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let first: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(first["type"], "run_start");
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

#[tokio::test]
async fn invalid_conversation_returns_error() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    wait_for_server(&server.base_url).await;

    let client = wreq::Client::new();
    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "hello",
            "workspace": server.workspace.path().to_str().unwrap(),
            "conversation": "main/api",
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
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("\"tool_result\"")
        });
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

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start_deferred(config).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_can_target_new_non_main_conversation() {
    let mock = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&mock, "Hello review!");

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    wait_for_server(&server.base_url).await;

    let client = wreq::Client::new();
    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "hello review",
            "workspace": server.workspace.path().to_str().unwrap(),
            "conversation": "review",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let lines = body
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(lines
        .iter()
        .any(|value| value["type"] == "done" && value["conversation"] == "review"));

    let sessions =
        kuku::session::list_sessions(server.home.path(), Some(server.workspace.path())).unwrap();
    let session_id = sessions.first().unwrap().session_id.clone();
    let events_path = kuku::session::session_events_path(
        server.home.path(),
        server.workspace.path(),
        &session_id,
    )
    .unwrap();
    let events = kuku::event::EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| matches!(&event.payload, kuku::event::EventPayload::ConversationOpened { conversation, .. } if conversation == "review")));
    assert!(events.iter().any(|event| matches!(&event.payload, kuku::event::EventPayload::MessageUser { conversation, text, .. } if conversation == "review" && text == "hello review")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_can_target_existing_non_main_conversation() {
    let mock = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&mock, "Hello again review!");

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;
    wait_for_server(&server.base_url).await;

    let client = wreq::Client::new();
    let first = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "first review",
            "workspace": server.workspace.path().to_str().unwrap(),
            "conversation": "review",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let _ = first.text().await.unwrap();

    let sessions =
        kuku::session::list_sessions(server.home.path(), Some(server.workspace.path())).unwrap();
    let session_id = sessions.first().unwrap().session_id.clone();

    let second = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "second review",
            "workspace": server.workspace.path().to_str().unwrap(),
            "session_id": session_id,
            "conversation": "review",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    let body = second.text().await.unwrap();
    let lines = body
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(lines
        .iter()
        .any(|value| value["type"] == "done" && value["conversation"] == "review"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn running_input_retry_returns_original_receipt_without_another_slot() {
    let mock = mock_provider::start_mock_provider().await;
    let model = mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200).delay(std::time::Duration::from_millis(700)).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"slow-dedup","type":"message","role":"assistant","content":[{"type":"text","text":"Original input only."}],
            "stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let server = common::TestServer::start(mock_provider::make_test_config(mock.port())).await;
    *server.state.run_manager.lock().await = kuku_server::run_manager::RunManager::new(1);
    let client = wreq::Client::new();
    let url = format!("{}/runs", server.base_url);
    let payload = serde_json::json!({
        "prompt":"keep original","workspace":server.workspace.path(),"session_id":"retry-running",
        "command_id":"original-input","connection":"anthropic","model":"fixture-model"
    });
    let response = client.post(&url).json(&payload).send().await.unwrap();
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let started = next_event_of_type(&mut stream, &mut buffer, "run_start").await;
    let duplicate: serde_json::Value = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, duplicate["ok"]);
    assert_eq!(true, duplicate["replayed"]);
    assert_eq!(started["receipt"], duplicate["receipt"]);
    assert_eq!(started["run_id"], duplicate["run_id"]);
    let mut conflicting = payload.clone();
    conflicting["prompt"] = serde_json::json!("changed input");
    let rejected: serde_json::Value = client
        .post(&url)
        .json(&conflicting)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(false, rejected["ok"]);
    conflicting = payload.clone();
    conflicting["model"] = serde_json::json!("changed-model");
    let rejected: serde_json::Value = client
        .post(&url)
        .json(&conflicting)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(false, rejected["ok"]);
    assert_eq!(
        1,
        server.state.run_manager.lock().await.active_run_ids().len()
    );
    let terminal = next_terminal_event(&mut stream, &mut buffer).await;
    assert_eq!("done", terminal["type"]);
    assert_eq!("fixture-model", terminal["config"]["model"]);
    model.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_input_retry_keeps_its_request_while_another_agent_uses_capacity() {
    let mock = mock_provider::start_mock_provider().await;
    let asked = mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").body_contains("wait for data");
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"ask-dedup","type":"message","role":"assistant",
            "content":[{"type":"tool_use","id":"wait-dedup","name":"ask_user","input":{"question":"Which value?"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let busy=mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").body_contains("hold capacity");
        then.status(200).delay(std::time::Duration::from_millis(700)).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"busy-dedup","type":"message","role":"assistant","content":[{"type":"text","text":"Busy finished."}],
            "stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let mut server = common::TestServer::start(mock_provider::make_test_config(mock.port())).await;
    *server.state.run_manager.lock().await = kuku_server::run_manager::RunManager::new(1);
    let client = wreq::Client::new();
    let url = format!("{}/runs", server.base_url);
    let payload = serde_json::json!({"prompt":"wait for data","workspace":server.workspace.path(),"session_id":"retry-waiting","command_id":"waiting-input"});
    let response = client.post(&url).json(&payload).send().await.unwrap();
    let mut waiting = response.bytes_stream();
    let mut buffer = Vec::new();
    let started = next_event_of_type(&mut waiting, &mut buffer, "run_start").await;
    let question = next_event_of_type(&mut waiting, &mut buffer, "question").await;
    let response=client.post(&url).json(&serde_json::json!({
        "prompt":"hold capacity","workspace":server.workspace.path(),"session_id":"retry-waiting","agent":"busy"
    })).send().await.unwrap();
    let mut running = response.bytes_stream();
    let mut running_buffer = Vec::new();
    let _ = next_event_of_type(&mut running, &mut running_buffer, "run_start").await;
    let duplicate: serde_json::Value = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, duplicate["replayed"]);
    assert_eq!(started["receipt"], duplicate["receipt"]);
    let mut conflict = payload.clone();
    conflict["prompt"] = serde_json::json!("changed while waiting");
    let rejected: serde_json::Value = client
        .post(&url)
        .json(&conflict)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(false, rejected["ok"]);
    let mut new_input = payload.clone();
    new_input["command_id"] = serde_json::json!("fresh-while-waiting");
    let rejected: serde_json::Value = client
        .post(&url)
        .json(&new_input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(false, rejected["ok"]);
    let path = kuku::session::session_events_path(
        server.home.path(),
        server.workspace.path(),
        "retry-waiting",
    )
    .unwrap();
    let agent = kuku::Session::read_agents(&path)
        .unwrap()
        .into_iter()
        .find(|agent| agent.address == "main")
        .unwrap();
    assert_eq!(kuku::AgentStatus::Waiting, agent.status);
    assert_eq!(
        question["id"].as_str(),
        agent.question.as_ref().map(|request| request.id.as_str())
    );
    assert_eq!(0, agent.pending_inputs);
    let terminal = next_terminal_event(&mut running, &mut running_buffer).await;
    assert_eq!("done", terminal["type"]);
    drop(waiting);
    server.restart().await;
    let reopened_url = format!("{}/runs", server.base_url);
    let duplicate: serde_json::Value = client
        .post(&reopened_url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, duplicate["replayed"]);
    assert_eq!(started["receipt"], duplicate["receipt"]);
    assert!(duplicate["run_id"].is_null());
    assert!(server
        .state
        .run_manager
        .lock()
        .await
        .active_run_ids()
        .is_empty());
    server.close().await;
    asked.assert_hits(1);
    busy.assert_hits(1);
}

#[tokio::test(flavor = "current_thread")]
async fn input_retry_validation_leaves_unadmitted_work_idle_until_fresh_input() {
    let mock = mock_provider::start_mock_provider().await;
    let model=mock.mock(|when,then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"pending","type":"message","role":"assistant","content":[{"type":"text","text":"pending work advanced explicitly"}],
            "stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config.clone()).await;
    let mut options =
        kuku::SessionOptions::new(server.workspace.path(), server.home.path(), config);
    options.session_id = Some("unadmitted-retry".into());
    let session = kuku::Session::open(options).await.unwrap();
    session
        .create_agent(
            "create:pending-input",
            "main",
            kuku::AgentConfig {
                connection: Some("anthropic".into()),
                model: Some("fixture-model".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut input = kuku::Input::new("pending-input", "main", "pending authorized work");
    input.source = kuku::InputSource::Host("http".into());
    let receipt = session.send(input).unwrap();
    let path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    // NOTE: Cut after the durable receipt to model a crash before admission.
    server.retain_receipt_prefix("unadmitted-retry", &receipt);
    let client = wreq::Client::new();
    let url = format!("{}/runs", server.base_url);
    let mut payload = serde_json::json!({
        "prompt":"pending authorized work","workspace":server.workspace.path(),"session_id":"unadmitted-retry",
        "command_id":"pending-input","connection":"anthropic","model":"changed-model"
    });
    let invalid: serde_json::Value = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(false, invalid["ok"]);
    let snapshot = kuku::Session::read_agents(&path).unwrap().pop().unwrap();
    assert_eq!(kuku::AgentStatus::Idle, snapshot.status);
    assert_eq!(1, snapshot.pending_inputs);
    assert_eq!(None, snapshot.turn);
    payload["model"] = serde_json::json!("fixture-model");
    let duplicate: serde_json::Value = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, duplicate["replayed"]);
    assert_eq!(serde_json::to_value(receipt).unwrap(), duplicate["receipt"]);
    assert_eq!(
        kuku::AgentStatus::Idle,
        kuku::Session::read_agents(&path).unwrap()[0].status
    );
    model.assert_hits(0);
    payload["command_id"] = serde_json::json!("explicit-fresh-input");
    payload["prompt"] = serde_json::json!("advance pending work with explicit fresh input");
    let completed = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        completed
            .lines()
            .any(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "done"),
        "{completed}"
    );
    model.assert_hits(1);
}
