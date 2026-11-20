use std::time::Duration;

use axum::body::Body;
use axum::routing::post;
use axum::Router;
use kuku::event::{EventPayload, EventStore};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{
    activity_snapshot, agent_summary, mock_provider, observe_agent_activity, seek_frame,
    text_response, TestServer,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_preserves_only_visible_text_once_and_keeps_it_out_of_the_next_request() {
    let (port, mut requests, provider) = interruptible_provider().await;
    let mut server = TestServer::start(mock_provider::make_test_config(port)).await;
    let id = "interrupted-reply";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"setup","text":"parent setup","create":{}}),
        )
        .await;
    server
        .wait_for_snapshot(id, "main", |snapshot| snapshot["agent"]["status"] == "idle")
        .await;
    server.submit(id, json!({"type":"create_agent","command_id":"create-peer","agent":"reviewer","config":{}})).await;

    let cursor = server.snapshot(id, "reviewer").await["cursor"]
        .as_u64()
        .unwrap();
    let mut observe = observe_agent_activity(&server, id, "reviewer", cursor)
        .await
        .bytes_stream();
    let mut buffer = Vec::new();
    let initial = text_response("");
    let message_start = &initial[..initial.find("event: content_block_start").unwrap()];
    let private_reason = "reasoning fixture must remain private";
    let thinking = |index| {
        sse(
            "content_block_start",
            json!({
                "index":index,"content_block":{"type":"thinking","thinking":private_reason}
            }),
        )
    };
    let request = json!({"type":"send","command_id":"work","agent":"reviewer","text":"write the first reply"});
    server.submit(id, request).await;
    let (_, send) = next_request(&server, id, &mut requests).await;
    seek_frame(
        &mut observe,
        &mut buffer,
        "prepared first request",
        |frame| frame["kind"] == "runtime.step_prepared" && frame["agent"] == "reviewer",
    )
    .await;
    send.send(format!("{message_start}{}", thinking(0)))
        .await
        .unwrap();
    let next_thinking = |frame: &Value| frame["event"]["type"] == "thinking";
    let frame = seek_frame(
        &mut observe,
        &mut buffer,
        "reasoning activity",
        next_thinking,
    )
    .await;
    assert_eq!(json!({"type":"thinking"}), frame["event"]);
    assert_eq!(
        "thinking",
        activity_snapshot(&server, id, "reviewer").await["progress"]["phase"]
    );

    let text = "界".repeat(22_000);
    send.send(sse(
        "content_block_start",
        json!({
            "index":1,"content_block":{"type":"text","text":text}
        }),
    ))
    .await
    .unwrap();
    seek_frame(&mut observe, &mut buffer, "visible reply text", |frame| {
        frame["event"]["type"] == "text"
    })
    .await;
    send.send(sse("content_block_start", json!({
        "index":2,"content_block":{"type":"tool_use","id":"unstarted","name":"read_file","input":{}}
    }))).await.unwrap();
    let arguments = seek_frame(
        &mut observe,
        &mut buffer,
        "tool argument activity",
        |frame| frame["event"]["type"] == "tool_arguments",
    )
    .await;
    assert_eq!(json!({"type":"tool_arguments"}), arguments["event"]);
    send.send(thinking(3)).await.unwrap();
    seek_frame(
        &mut observe,
        &mut buffer,
        "reasoning after visible text",
        next_thinking,
    )
    .await;

    let snapshot = activity_snapshot(&server, id, "reviewer").await;
    let preview = &snapshot["reply_preview"];
    let prefix = &text[..text.floor_char_boundary(64 * 1024)];
    assert_eq!("thinking", preview["phase"]);
    assert_eq!(prefix, preview["text"]);
    assert_eq!(true, preview["truncated"]);
    let parent = activity_snapshot(&server, id, "main").await;
    assert_eq!(
        "thinking",
        agent_summary(&parent, "reviewer")["progress"]["phase"]
    );

    drop(observe);
    buffer.clear();
    let mut observe = observe_agent_activity(
        &server,
        id,
        "reviewer",
        snapshot["cursor"].as_u64().unwrap(),
    )
    .await
    .bytes_stream();
    let seed = seek_frame(
        &mut observe,
        &mut buffer,
        "current prefix on attachment",
        |frame| frame["event"]["type"] == "reply_preview",
    )
    .await;
    assert_eq!("thinking", seed["event"]["phase"]);
    assert_eq!(prefix, seed["event"]["content"]);

    let stop = json!({"type":"cancel","command_id":"escape","agent":"reviewer","execution_id":preview["execution_id"]});
    let receipt = server.submit(id, stop.clone()).await;
    let frame = seek_frame(
        &mut observe,
        &mut buffer,
        "committed interruption",
        |frame| frame["entry"]["status"] == "interrupted",
    )
    .await;
    let entry = &frame["entry"];
    assert_eq!(preview["execution_id"], entry["execution_id"]);
    assert_eq!(preview["turn"], entry["turn"]);
    assert_eq!(1, entry["turn_number"]);
    assert_eq!(prefix, entry["text"]);
    assert_eq!(true, entry["truncated"]);
    assert_eq!(entry["id"], receipt["receipt"]["event_id"]);
    assert_eq!(receipt["receipt"], server.submit(id, stop).await["receipt"]);
    drop(observe);
    let stopped = activity_snapshot(&server, id, "reviewer").await;
    assert_eq!("stopped", stopped["agent"]["status"]);
    assert!(stopped["agent"]["last_output"].is_null());
    assert!(stopped["reply_preview"].is_null());

    server.restart().await;
    let history: Value = server
        .session_request(id, "history", &[("agent", "reviewer")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        json!(["user", "status"]),
        json!(history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["type"].clone())
            .collect::<Vec<_>>())
    );
    assert_eq!(*entry, history["entries"][1]);
    let journal = EventStore::replay(server.events_path(id)).unwrap();
    assert!(journal.iter().any(|event| matches!(&event.payload,
        EventPayload::TurnCancelled { stop_event_id: Some(stop_id), .. } if Some(*stop_id) == entry["id"].as_u64())));
    assert!(!journal
        .iter()
        .any(|event| event.payload.kind_name() == "runtime.tool_dispatched"));
    assert!(!serde_json::to_string(&journal)
        .unwrap()
        .contains(private_reason));

    server.submit(id, json!({"type":"send","command_id":"fresh","agent":"reviewer","text":"start a new reply"})).await;
    let (request, next) = next_request(&server, id, &mut requests).await;
    assert!(!request.contains('界'));
    assert!(!request.contains(private_reason));
    next.send(format!("{message_start}{}", thinking(0)))
        .await
        .unwrap();
    let snapshot = server.snapshot(id, "reviewer").await;
    let mut observe = observe_agent_activity(
        &server,
        id,
        "reviewer",
        snapshot["cursor"].as_u64().unwrap(),
    )
    .await
    .bytes_stream();
    let evidence = seek_frame(
        &mut observe,
        &mut Vec::new(),
        "reasoning-only request",
        |frame| frame["event"]["type"] == "thinking" || frame["event"]["phase"] == "thinking",
    )
    .await;
    server.submit(id, json!({"type":"cancel","command_id":"escape-no-text","agent":"reviewer","execution_id":evidence["execution_id"]})).await;
    let history: Value = server
        .session_request(id, "history", &[("agent", "reviewer"), ("limit", "1")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let last = &history["entries"][0];
    assert_eq!("interrupted", last["status"]);
    assert_eq!(2, last["turn_number"]);
    assert!(last["text"].is_null());
    provider.abort();
}

type StagedRequest = (String, mpsc::Sender<String>);

async fn next_request(
    server: &TestServer,
    id: &str,
    requests: &mut mpsc::Receiver<StagedRequest>,
) -> StagedRequest {
    let request = tokio::time::timeout_at(server.deadline, requests.recv()).await;
    assert!(
        request.is_ok(),
        "provider did not receive a request; snapshot: {:?}",
        tokio::time::timeout(Duration::from_secs(1), server.snapshot(id, "reviewer"))
            .await
            .map(|snapshot| super::common::snapshot_summary(&snapshot))
    );
    request.unwrap().expect("provider request channel closed")
}

async fn interruptible_provider() -> (
    u16,
    mpsc::Receiver<StagedRequest>,
    tokio::task::JoinHandle<()>,
) {
    let (requests, receive) = mpsc::channel(2);
    let app = Router::new().route(
        "/v1/messages",
        post(move |body: String| {
            let requests = requests.clone();
            async move {
                let body = if body.contains("parent setup") {
                    Body::from(text_response("Parent ready."))
                } else {
                    let (send, read) = mpsc::channel(1);
                    requests.send((body, send)).await.unwrap();
                    Body::from_stream(tokio_stream::StreamExt::map(
                        ReceiverStream::new(read),
                        Ok::<String, std::convert::Infallible>,
                    ))
                };
                ([("content-type", "text/event-stream")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, receive, handle)
}

fn sse(kind: &str, mut data: Value) -> String {
    data["type"] = json!(kind);
    format!("event: {kind}\ndata: {data}\n\n")
}
