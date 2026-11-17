mod common;

use std::time::Duration;

use common::mock_provider;
use common::stream::next_event_of_type;
use httpmock::Method;
use kuku::session::{AgentStatus, Session};
use serde_json::{json, Value};

fn request_contains_tool_result(request: &httpmock::prelude::HttpMockRequest) -> bool {
    String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("\"tool_result\"")
}

fn lines(body: &str) -> Vec<Value> {
    body.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnecting_http_observer_preserves_execution_and_durable_result() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages").matches(|request| !request_contains_tool_result(request));
        then.status(200).body(mock_provider::anthropic_sse_response(json!({
            "id":"write","type":"message","role":"assistant",
            "content":[{"type":"tool_use","id":"write-proof","name":"write_file","input":{"path":"proof.txt","content":"persisted\n","brief":"write proof"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":8}
        })));
    });
    let continuation = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages").matches(request_contains_tool_result).body_contains("write-proof");
        then.status(200).body(mock_provider::anthropic_sse_response(json!({
            "id":"done","type":"message","role":"assistant","content":[{"type":"text","text":"Proof committed."}],
            "stop_reason":"end_turn","usage":{"input_tokens":9,"output_tokens":4}
        })));
    });
    let server = common::TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let client = wreq::Client::new();
    let response = client
        .post(format!("{}/runs", server.base_url))
        .json(&json!({
            "prompt":"write proof", "workspace":server.workspace.path(),
        }))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let start = next_event_of_type(&mut stream, &mut Vec::new(), "run_start").await;
    drop(stream);
    let session_id = start["session_id"].as_str().unwrap();
    let path =
        kuku::session::session_events_path(server.home.path(), server.workspace.path(), session_id)
            .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let agents = Session::read_agents(&path).unwrap();
            if agents.first().is_some_and(|agent| {
                agent.status == AgentStatus::Idle
                    && agent.last_output.as_deref() == Some("Proof committed.")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("disconnected observer must not stop the Session executor");
    assert_eq!(
        "persisted\n",
        std::fs::read_to_string(server.workspace.path().join("proof.txt")).unwrap()
    );
    continuation.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_reopens_independent_agents_with_exact_same_protocol_connections() {
    let first = mock_provider::start_mock_provider().await;
    let second = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&first, "first result");
    let second_response = second.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages").body_contains("second material").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("first material")
        });
        then.status(200).body(mock_provider::anthropic_sse_response(json!({
            "id":"second","type":"message","role":"assistant","content":[{"type":"text","text":"second result"}],
            "stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let mut config = mock_provider::make_test_config(first.port());
    let mut connection = config.providers["anthropic"].clone();
    connection.base_url = second.base_url();
    config.providers.insert("secondary".into(), connection);
    let server = common::TestServer::start(config).await;
    let client = wreq::Client::new();
    let first_body = client.post(format!("{}/runs",server.base_url)).json(&json!({
        "prompt":"first material","workspace":server.workspace.path(),"agent":"writer","connection":"anthropic","model":"fixture-model"
    })).send().await.unwrap().text().await.unwrap();
    let first_events = lines(&first_body);
    let session_id = first_events[0]["session_id"].as_str().unwrap();
    let second_body = client
        .post(format!("{}/runs", server.base_url))
        .json(&json!({
            "prompt":"second material","workspace":server.workspace.path(),"session_id":session_id,
            "agent":"reviewer","connection":"secondary","model":"fixture-model"
        }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let second_events = lines(&second_body);
    assert_eq!(
        Some("done"),
        second_events
            .last()
            .and_then(|event| event["type"].as_str()),
        "{second_body}"
    );
    assert_eq!(
        "secondary",
        second_events.last().unwrap()["config"]["connection"]
    );
    let path =
        kuku::session::session_events_path(server.home.path(), server.workspace.path(), session_id)
            .unwrap();
    let agents = Session::read_agents(&path).unwrap();
    assert_eq!(2, agents.len());
    assert_eq!(Some("secondary"), agents[0].config.connection.as_deref());
    assert_eq!(Some("anthropic"), agents[1].config.connection.as_deref());
    second_response.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_shutdown_preserves_waiting_question_for_reopen() {
    let mock = mock_provider::start_mock_provider().await;
    let request = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages");
        then.status(200).body(mock_provider::anthropic_sse_response(json!({
            "id":"ask","type":"message","role":"assistant",
            "content":[{"type":"tool_use","id":"ask-name","name":"ask_user","input":{"question":"Which name?"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let config = mock_provider::make_test_config(mock.port());
    let mut server = common::TestServer::start(config.clone()).await;
    let response = wreq::Client::new()
        .post(format!("{}/runs", server.base_url))
        .json(&json!({
            "prompt":"ask for a name","workspace":server.workspace.path(),
        }))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let start = next_event_of_type(&mut stream, &mut buffer, "run_start").await;
    let question = next_event_of_type(&mut stream, &mut buffer, "question").await;
    server.close().await;
    let mut options =
        kuku::SessionOptions::new(server.workspace.path(), server.home.path(), config);
    options.session_id = Some(start["session_id"].as_str().unwrap().into());
    let reopened = Session::open(options).await.unwrap();
    let agent = reopened.agent("main").unwrap();
    assert_eq!(AgentStatus::Waiting, agent.status);
    assert_eq!(
        question["id"].as_str(),
        agent.question.as_ref().map(|question| question.id.as_str())
    );
    reopened.close().await.unwrap();
    request.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_local_command_ids_do_not_replace_other_session_controls() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages");
        then.status(200).delay(Duration::from_millis(250)).body(mock_provider::anthropic_sse_response(json!({
            "id":"delayed","type":"message","role":"assistant","content":[{"type":"text","text":"completed independently"}],
            "stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let server = common::TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let client = wreq::Client::new();
    let mut streams = Vec::new();
    let mut starts = Vec::new();
    for session_id in ["first-session", "second-session"] {
        let response = client.post(format!("{}/runs",server.base_url)).json(&json!({
            "prompt":"run independently","workspace":server.workspace.path(),"session_id":session_id,"command_id":"same-command"
        })).send().await.unwrap();
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        starts.push(next_event_of_type(&mut stream, &mut buffer, "run_start").await);
        streams.push((stream, buffer));
    }
    assert_ne!(starts[0]["run_id"], starts[1]["run_id"]);
    assert_eq!(
        2,
        server.state.run_manager.lock().await.active_run_ids().len()
    );
    let cancelled: Value = client
        .delete(format!(
            "{}/runs/{}",
            server.base_url,
            starts[0]["run_id"].as_str().unwrap()
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, cancelled["ok"]);
    let (stream, buffer) = &mut streams[1];
    let terminal = common::stream::next_terminal_event(stream, buffer).await;
    assert_eq!("done", terminal["type"]);
    assert_eq!("completed independently", terminal["text"]);
}
