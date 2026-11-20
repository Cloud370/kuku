mod common;

use std::collections::VecDeque;
use std::future::{ready, Future};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::routing::post;
use axum::{Json, Router};
use common::mock_provider::{self, anthropic_sse_response, text_response, tool_response};
use common::TestServer;
use kuku::event::{EventPayload, EventStore};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

async fn start_provider<F: Future<Output = String> + Send + 'static>(
    responses: impl IntoIterator<Item = F>,
) -> (u16, tokio::task::JoinHandle<()>) {
    let responses = Arc::new(Mutex::new(responses.into_iter().collect::<VecDeque<_>>()));
    let router = Router::new().route(
        "/v1/messages",
        post(move |Json(request): Json<Value>| {
            assert!(request["messages"].is_array(), "complete model request");
            let responses = responses.clone();
            async move {
                let response = responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("scheduled model response");
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    response.await,
                )
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (port, task)
}

#[tokio::test]
async fn provider_fixture_reads_the_complete_request_before_responding() {
    let (requested, mut received) = oneshot::channel();
    let (port, provider) = start_provider([async move {
        requested.send(()).unwrap();
        text_response("complete request received")
    }])
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let body = br#"{"model":"fixture","messages":[{"role":"user","content":"fixture request"}]}"#;
        let headers = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        let mut interim = Vec::new();
        while !interim.ends_with(b"\r\n\r\n") {
            interim.push(stream.read_u8().await.expect("provider closed before accepting the request body"));
            assert!(interim.len() < 1024);
        }
        assert!(interim.starts_with(b"HTTP/1.1 100 "), "provider replied before accepting the request body: {}", String::from_utf8_lossy(&interim));
        assert!(matches!(received.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
        stream.write_all(body).await.unwrap();
        received.await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200 "));
        assert!(response.windows(b"complete request received".len()).any(|part| part == b"complete request received"));
    })
    .await
    .expect("provider fixture did not consume the complete request");
    provider.abort();
}

async fn send(server: &TestServer, id: &str) {
    server
        .submit(
            id,
            json!({"type":"send", "command_id":"prompt", "text":"Run the fixture", "create":{}}),
        )
        .await;
}

async fn history(server: &TestServer, id: &str, query: &[(&str, &str)]) -> Vec<Value> {
    let response = server
        .session_request(id, "history", query)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    response.json::<Value>().await.unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn summaries(entries: &[Value]) -> Vec<&Value> {
    entries
        .iter()
        .filter(|entry| entry.get("turn_summary").is_some())
        .collect()
}

fn deferred_writes(count: usize, usage: Value) -> String {
    let calls = (0..count)
        .map(|index| {
            json!({"type":"tool_use", "id":format!("check-{index}"), "name":"write_file",
                "input":{"brief":"Fixture write", "path":"permission-fixture.txt", "content":"fixture\n"}})
        })
        .collect::<Vec<_>>();
    anthropic_sse_response(json!({
        "id":"fixture-tool", "type":"message", "role":"assistant",
        "content":calls,
        "stop_reason":"tool_use", "usage":usage,
    }))
}

#[tokio::test]
async fn permission_resume_keeps_the_turn_and_replays_one_summary_after_restart() {
    let reply = anthropic_sse_response(json!({
        "id":"fixture-text", "type":"message", "role":"assistant",
        "content":[{"type":"text", "text":"Fixture answered"}], "stop_reason":"end_turn",
        "usage":{"input_tokens":15,"output_tokens":7,"cache_read_input_tokens":5},
    }));
    let (port, provider) = start_provider(
        [
            deferred_writes(
                1,
                json!({"input_tokens":40,"output_tokens":10,"cache_read_input_tokens":60}),
            ),
            reply,
        ]
        .map(ready),
    )
    .await;
    let mut server = TestServer::start_deferred(mock_provider::make_test_config(port)).await;
    let id = "permission-summary";
    send(&server, id).await;
    let waiting = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["permission"].is_object()
        })
        .await;
    let agent = &waiting["agent"];
    let started = agent["turn_started_at_ms"].as_u64().unwrap();
    let turn = agent["turn"].as_u64().unwrap();
    assert!(agent["turn_elapsed_ms"].is_null());
    assert!(summaries(&history(&server, id, &[]).await).is_empty());
    let mut stream = server
        .session_request(id, "observe", &[("agent", "main"), ("after", "0")])
        .send()
        .await
        .unwrap()
        .bytes_stream();
    let mut buffer = Vec::new();
    let accepted = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame =
                common::stream::next_json_line(&mut stream, &mut buffer, "first admission").await;
            if frame["operation"] == "inputs_accepted" {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(started, accepted["turn_started_at_ms"]);
    assert_eq!(turn, accepted["turn"]);

    server.submit(id, json!({"type":"reply_permission", "command_id":"deny", "request_id":agent["permission"]["id"], "allow":false})).await;
    let live = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame =
                common::stream::next_json_line(&mut stream, &mut buffer, "terminal summary").await;
            if frame["entry"]["type"] == "turn_summary" {
                break frame["entry"].clone();
            }
        }
    })
    .await
    .unwrap();
    drop(stream);
    assert_eq!(turn, live["turn"]);
    assert_eq!(1, live["turn_number"]);
    let stats = &live["turn_summary"];
    assert_eq!(1, stats["tool_calls"]);
    assert_eq!(120, stats["input_tokens"]);
    assert_eq!(17, stats["output_tokens"]);
    let cache = stats["cache_hit_percent"].as_f64().unwrap();
    assert!((cache - 100.0 * 65.0 / 120.0).abs() < 1e-9);
    let elapsed = stats["elapsed_ms"].as_u64().unwrap();
    assert_eq!(vec![&live], summaries(&history(&server, id, &[]).await));
    let before = (live["id"].as_u64().unwrap() + 1).to_string();
    assert_eq!(
        vec![live.clone()],
        history(&server, id, &[("before", &before), ("limit", "1")]).await
    );

    server.restart().await;
    assert_eq!(vec![&live], summaries(&history(&server, id, &[]).await));
    let reopened = server.snapshot(id, "main").await;
    assert_eq!(started, reopened["agent"]["turn_started_at_ms"]);
    assert_eq!(elapsed, reopened["agent"]["turn_elapsed_ms"]);
    assert!(!server
        .workspace
        .path()
        .join("permission-fixture.txt")
        .exists());
    provider.abort();
}

#[tokio::test]
async fn explicit_stop_attaches_statistics_to_its_only_interruption_row() {
    const CALLS: usize = 3;
    let (port, provider) = start_provider([ready(deferred_writes(
        CALLS,
        json!({"input_tokens":5,"output_tokens":6}),
    ))])
    .await;
    let mut server = TestServer::start_deferred(mock_provider::make_test_config(port)).await;
    let id = "stopped-summary";
    send(&server, id).await;
    let waiting = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["permission"].is_object()
        })
        .await;
    let agent = &waiting["agent"];
    server
        .submit(
            id,
            json!({"type":"cancel", "command_id":"stop", "execution_id":agent["execution_id"]}),
        )
        .await;
    let entries = history(&server, id, &[]).await;
    let reduced = summaries(&entries);
    assert_eq!(1, reduced.len());
    let stopped = reduced[0];
    assert_eq!("status", stopped["type"]);
    assert_eq!("interrupted", stopped["status"]);
    assert_eq!(agent["turn"], stopped["turn"]);
    assert_eq!(CALLS, stopped["turn_summary"]["tool_calls"]);
    assert_eq!(5, stopped["turn_summary"]["input_tokens"]);
    assert_eq!(6, stopped["turn_summary"]["output_tokens"]);
    assert!(stopped["turn_summary"]["elapsed_ms"].is_u64());
    assert_eq!(
        1,
        entries
            .iter()
            .filter(|entry| entry["type"] == "status")
            .count()
    );
    server.restart().await;
    assert_eq!(vec![stopped], summaries(&history(&server, id, &[]).await));
    assert!(!server
        .workspace
        .path()
        .join("permission-fixture.txt")
        .exists());
    provider.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_racing_a_model_response_counts_only_calls_committed_before_it() {
    let (requested, received) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    const CALLS: usize = 20;
    let (port, provider) = start_provider([async move {
        requested.send(()).unwrap();
        gate.await.unwrap();
        deferred_writes(CALLS, json!({"input_tokens":5,"output_tokens":6}))
    }])
    .await;
    let server = TestServer::start_deferred(mock_provider::make_test_config(port)).await;
    let id = "stop-tool-commit-race";
    send(&server, id).await;
    let admission = tokio::time::timeout_at(server.deadline, received).await;
    assert!(
        matches!(admission, Ok(Ok(()))),
        "model fixture was not reached: {admission:?}; snapshot: {:?}",
        tokio::time::timeout(Duration::from_secs(1), server.snapshot(id, "main"))
            .await
            .map(|snapshot| common::snapshot_summary(&snapshot))
    );
    let running = server.snapshot(id, "main").await;
    let agent = &running["agent"];
    let turn = agent["turn"].as_u64().unwrap();
    let cancel = server.submit(
        id,
        json!({"type":"cancel", "command_id":"stop", "execution_id":agent["execution_id"]}),
    );
    let (receipt, _) = tokio::join!(cancel, async { release.send(()).unwrap() });
    let stop = receipt["receipt"]["event_id"].as_u64().unwrap();
    let journal = EventStore::replay(server.events_path(id)).unwrap();
    let committed = journal
        .iter()
        .filter(
            |event| matches!(event.payload, EventPayload::ToolCall { turn: id, .. } if id == turn),
        )
        .collect::<Vec<_>>();
    assert!(committed.is_empty() || committed.len() == CALLS);
    assert!(committed.iter().all(|call| call.id < stop));

    let entries = history(&server, id, &[]).await;
    let summaries = summaries(&entries);
    assert_eq!(1, summaries.len());
    assert_eq!(stop, summaries[0]["id"]);
    assert_eq!(committed.len(), summaries[0]["turn_summary"]["tool_calls"]);
    provider.abort();
}

#[tokio::test]
async fn a_completed_question_turn_and_its_follow_up_have_distinct_summaries() {
    let (port, provider) = start_provider(
        [
            tool_response("ask_user", "ask", json!({"question":"Which fact?"})),
            text_response("Answer processed"),
        ]
        .map(ready),
    )
    .await;
    let server = TestServer::start(mock_provider::make_test_config(port)).await;
    let id = "question-summary";
    send(&server, id).await;
    let waiting = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["status"] == "waiting"
        })
        .await;
    let entries = history(&server, id, &[]).await;
    let previous = summaries(&entries);
    assert_eq!(1, previous.len());
    assert_eq!(1, previous[0]["turn_summary"]["tool_calls"]);
    server.submit(id, json!({"type":"reply_question", "command_id":"answer", "request_id":waiting["agent"]["question"]["id"], "text":"Use the recorded fact"})).await;
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "Answer processed"
        })
        .await;
    let entries = history(&server, id, &[]).await;
    let final_summaries = summaries(&entries);
    assert_eq!(2, final_summaries.len());
    assert_eq!(previous[0], final_summaries[0]);
    assert_ne!(final_summaries[0]["turn"], final_summaries[1]["turn"]);
    assert_eq!(2, final_summaries[1]["turn_number"]);
    assert_eq!(0, final_summaries[1]["turn_summary"]["tool_calls"]);
    provider.abort();
}

#[tokio::test]
async fn a_provider_error_keeps_the_turn_summary_with_unknown_usage() {
    let provider = mock_provider::start_mock_provider().await;
    let call = provider.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(400).body(r#"{"type":"error","error":{"type":"invalid_request_error","message":"fixture rejected"}}"#);
    });
    let server = TestServer::start(mock_provider::make_test_config(provider.port())).await;
    let id = "error-summary";
    send(&server, id).await;
    let failed = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["status"] == "interrupted"
        })
        .await;
    let entries = history(&server, id, &[]).await;
    let summaries = summaries(&entries);
    assert_eq!(1, summaries.len());
    let stats = &summaries[0]["turn_summary"];
    assert_eq!(failed["agent"]["turn"], summaries[0]["turn"]);
    assert!(stats["elapsed_ms"].is_u64());
    assert!(stats["input_tokens"].is_null());
    assert!(stats["output_tokens"].is_null());
    assert!(stats["cache_hit_percent"].is_null());
    assert_eq!(0, stats["tool_calls"]);
    call.assert_hits(1);
}
