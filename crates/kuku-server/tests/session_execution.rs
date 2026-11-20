mod common;
#[path = "session_execution/interruptions.rs"]
mod interruptions;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::routing::post;
use axum::Router;
use common::mock_provider::{self, text_response, tool_response, ControlledProvider};
use common::TestServer;
use httpmock::Method;
use kuku::event::{EventPayload, EventStore};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::wrappers::ReceiverStream;

const TOOL_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_observer_gets_selected_text_and_bounded_activity_from_a_streaming_peer() {
    let text = "界".repeat(400);
    let between = " text produced between snapshot and attach.";
    let tail = " Final delta after attaching.";
    let (port, begin, additional, final_delta, finish, provider) =
        paused_peer_reply(&text, between, tail).await;
    let mut server = TestServer::start(mock_provider::make_test_config(port)).await;
    let id = "peer-preview";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"initial","text":"parent setup","create":{}}),
        )
        .await;
    server
        .wait_for_snapshot(id, "main", |snapshot| snapshot["agent"]["status"] == "idle")
        .await;
    server.submit(id, json!({"type":"create_agent","command_id":"create-peer","agent":"reviewer","config":{}})).await;
    let snapshot = server.snapshot(id, "main").await;
    let cursor = snapshot["cursor"].as_u64().unwrap();
    server.submit(id, json!({"type":"send","command_id":"peer-input","agent":"reviewer","text":"hold peer stream"})).await;
    let mut parent = observe_agent_activity(&server, id, "main", cursor)
        .await
        .bytes_stream();
    let mut reviewer = observe_agent_activity(&server, id, "reviewer", cursor)
        .await
        .bytes_stream();
    let mut parent_buffer = Vec::new();
    let mut reviewer_buffer = Vec::new();
    let step =
        |frame: &Value| frame["agent"] == "reviewer" && frame["kind"] == "runtime.step_prepared";
    seek_frame(
        &mut parent,
        &mut parent_buffer,
        "peer Step on parent stream",
        step,
    )
    .await;
    seek_frame(
        &mut reviewer,
        &mut reviewer_buffer,
        "reviewer Step on selected stream",
        step,
    )
    .await;
    let running = activity_snapshot(&server, id, "main").await;
    let peer = agent_summary(&running, "reviewer");
    assert_eq!("running", peer["status"]);
    assert_eq!("waiting_for_model", peer["progress"]["phase"]);
    assert!(peer["last_output_preview"].is_null());
    begin.send(()).unwrap();

    let text_frame =
        |frame: &Value| frame["agent"] == "reviewer" && frame["event"]["type"] == "text";
    let related = seek_frame(
        &mut parent,
        &mut parent_buffer,
        "bounded initial peer text",
        text_frame,
    )
    .await;
    let selected = seek_frame(
        &mut reviewer,
        &mut reviewer_buffer,
        "selected initial text",
        text_frame,
    )
    .await;
    assert_eq!(peer["execution_id"], related["execution_id"]);
    assert_eq!(text.as_str(), selected["event"]["content"]);
    let preview = &text[..text.floor_char_boundary(512)];
    assert_eq!(preview, related["event"]["content"]);

    let snapshot = activity_snapshot(&server, id, "reviewer").await;
    assert_eq!(text.as_str(), snapshot["reply_preview"]["text"]);
    assert_eq!(
        peer["execution_id"],
        snapshot["reply_preview"]["execution_id"]
    );
    assert_eq!(1, snapshot["reply_preview"]["turn_number"]);
    assert_eq!(false, snapshot["reply_preview"]["truncated"]);
    let parent_again = activity_snapshot(&server, id, "main").await;
    assert!(parent_again["reply_preview"].is_null());
    assert_eq!(
        json!({"phase":"streaming","preview":preview}),
        agent_summary(&parent_again, "reviewer")["progress"]
    );
    let mut late_parent = observe_agent_activity(
        &server,
        id,
        "main",
        parent_again["cursor"].as_u64().unwrap(),
    )
    .await
    .bytes_stream();
    let mut late_parent_buffer = Vec::new();
    let recovered = seek_frame(
        &mut late_parent,
        &mut late_parent_buffer,
        "reviewer prefix on late parent attachment",
        |frame| frame["agent"] == "reviewer" && frame["event"]["type"] == "reply_preview",
    )
    .await;
    assert_eq!(preview, recovered["event"]["content"]);
    assert_eq!("", recovered["event"]["reasoning_preview"]);
    let after = snapshot["cursor"].as_u64().unwrap();
    additional.send(()).unwrap();
    let middle = seek_frame(
        &mut reviewer,
        &mut reviewer_buffer,
        "text before reattachment",
        text_frame,
    )
    .await;
    assert_eq!(between, middle["event"]["content"]);
    let late_middle = seek_frame(
        &mut late_parent,
        &mut late_parent_buffer,
        "first new delta after the peer prefix",
        text_frame,
    )
    .await;
    assert_eq!(between, late_middle["event"]["content"]);
    drop(late_parent);
    drop(reviewer);

    let mut reattached = observe_agent_activity(&server, id, "reviewer", after)
        .await
        .bytes_stream();
    let mut reattached_buffer = Vec::new();
    let prefix = seek_frame(
        &mut reattached,
        &mut reattached_buffer,
        "replacement prefix on reattachment",
        |frame| frame["agent"] == "reviewer" && frame["event"]["type"] == "reply_preview",
    )
    .await;
    assert_eq!(format!("{text}{between}"), prefix["event"]["content"]);
    assert_eq!(
        snapshot["reply_preview"]["execution_id"],
        prefix["execution_id"]
    );
    assert_eq!(snapshot["reply_preview"]["turn"], prefix["turn"]);
    final_delta.send(()).unwrap();
    let next = seek_frame(
        &mut reattached,
        &mut reattached_buffer,
        "new live text after reattachment",
        text_frame,
    )
    .await;
    assert_eq!(tail, next["event"]["content"]);
    finish.send(()).unwrap();

    let full = format!("{text}{between}{tail}");
    let committed = seek_frame(
        &mut reattached,
        &mut reattached_buffer,
        "committed reviewer reply",
        |frame| frame["agent"] == "reviewer" && frame["entry"]["type"] == "assistant",
    )
    .await;
    assert_eq!(full, committed["entry"]["text"]);
    drop(reattached);

    let end = seek_frame(
        &mut parent,
        &mut parent_buffer,
        "reviewer settlement on parent stream",
        |frame| frame["agent"] == "reviewer" && frame["operation"] == "turn_settled",
    )
    .await;
    assert_eq!("idle", end["settlement"]["status"]);
    assert_eq!(preview, end["settlement"]["output"]);
    assert!(end["entry"].is_null());
    drop(parent);
    assert!(activity_snapshot(&server, id, "reviewer").await["reply_preview"].is_null());
    server.restart().await;
    let reopened = server.snapshot(id, "main").await;
    let peer = agent_summary(&reopened, "reviewer");
    assert_eq!("idle", peer["status"]);
    assert_eq!(preview, peer["last_output_preview"]);
    assert!(peer["execution_id"].is_null());
    provider.abort();
}

async fn paused_peer_reply(
    text: &str,
    between: &str,
    tail: &str,
) -> (
    u16,
    oneshot::Sender<()>,
    oneshot::Sender<()>,
    oneshot::Sender<()>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let response = text_response(text);
    let split = response.find("event: content_block_stop").unwrap();
    let delta = |text: &str| {
        format!(
            "event: content_block_delta\ndata: {}\n\n",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})
        )
    };
    let (begin, wait_for_begin) = oneshot::channel();
    let (additional, wait_for_additional) = oneshot::channel();
    let (final_delta, wait_for_final_delta) = oneshot::channel();
    let (finish, wait_for_finish) = oneshot::channel();
    let pending = Arc::new(Mutex::new(Some((
        response[..split].to_owned(),
        delta(between),
        delta(tail),
        response[split..].to_owned(),
        wait_for_begin,
        wait_for_additional,
        wait_for_final_delta,
        wait_for_finish,
    ))));
    let app = Router::new().route(
        "/v1/messages",
        post(move |body: String| {
            let pending = pending.clone();
            async move {
                let body = if body.contains("hold peer stream") {
                    let (first, middle, tail, rest, begin, additional, final_delta, finish) =
                        pending.lock().await.take().unwrap();
                    let (send, read) = mpsc::channel(1);
                    tokio::spawn(async move {
                        if begin.await.is_ok() {
                            let _ = send
                                .send(Ok::<_, std::convert::Infallible>(Bytes::from(first)))
                                .await;
                        }
                        if additional.await.is_ok() {
                            let _ = send.send(Ok(Bytes::from(middle))).await;
                        }
                        if final_delta.await.is_ok() {
                            let _ = send.send(Ok(Bytes::from(tail))).await;
                        }
                        if finish.await.is_ok() {
                            let _ = send.send(Ok(Bytes::from(rest))).await;
                        }
                    });
                    Body::from_stream(ReceiverStream::new(read))
                } else {
                    Body::from(text_response("Parent ready."))
                };
                ([("content-type", "text/event-stream")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, begin, additional, final_delta, finish, handle)
}

async fn observe_agent_activity(
    server: &TestServer,
    id: &str,
    agent: &str,
    cursor: u64,
) -> wreq::Response {
    let response = server
        .session_request(
            id,
            "observe",
            &[
                ("agent", agent),
                ("after", &cursor.to_string()),
                ("include_agent_activity", "true"),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    response
}

async fn seek_frame(
    stream: &mut (impl tokio_stream::Stream<Item = Result<Bytes, wreq::Error>> + Unpin),
    buffer: &mut Vec<u8>,
    boundary: &str,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = common::stream::next_json_line(stream, buffer, boundary).await;
            if predicate(&frame) {
                return frame;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {boundary}"))
}

fn agent_summary<'a>(snapshot: &'a Value, address: &str) -> &'a Value {
    snapshot["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["address"] == address)
        .unwrap()
}

async fn activity_snapshot(server: &TestServer, session: &str, agent: &str) -> Value {
    let response = server
        .session_request(
            session,
            "snapshot",
            &[("agent", agent), ("progress", "true")],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    response.json().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observer_disconnect_leaves_exactly_one_durable_tool_result() {
    let mut provider = ControlledProvider::start().await;
    let server = TestServer::start(mock_provider::make_test_config(provider.port())).await;
    let id = "durable-result";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"command","text":"write proof","create":{}}),
        )
        .await;
    let call = provider.next().await;
    let first = server.snapshot(id, "main").await;
    let cursor = first["cursor"].as_u64().unwrap();
    let observer = server
        .session_request(
            id,
            "observe",
            &[("after", &cursor.to_string()), ("agent", "main")],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(200, observer.status());
    assert_eq!("application/x-ndjson", observer.headers()["content-type"]);
    drop(observer);
    call.reply(tool_response(
        "write_file",
        "write-proof",
        json!({
            "path":"proof.txt","content":"persisted\n","brief":"write proof"
        }),
    ));
    let continuation = provider.next().await;
    let result = continuation.request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == "write-proof")
        .expect("Committed write_file result is missing");
    assert_ne!(true, result["is_error"], "{result}");
    continuation.reply(text_response("Proof committed."));

    let end = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "Proof committed."
        })
        .await;
    assert_eq!(
        "persisted\n",
        std::fs::read_to_string(server.workspace.path().join("proof.txt")).unwrap()
    );
    let mut reconnected = server
        .session_request(
            id,
            "observe",
            &[("after", &cursor.to_string()), ("agent", "main")],
        )
        .send()
        .await
        .unwrap()
        .bytes_stream();
    let mut buffer = Vec::new();
    let mut results = 0;
    let mut previous = cursor;
    tokio::time::timeout(Duration::from_secs(3), async {
        while previous < end["cursor"].as_u64().unwrap() {
            let item =
                common::stream::next_json_line(&mut reconnected, &mut buffer, "reconnect").await;
            if item["type"] != "committed" {
                continue;
            }
            let current = item["id"].as_u64().unwrap();
            assert!(current > previous, "reconnected cursor moved backward");
            previous = current;
            if item["entry"]["type"] == "tool_result"
                && item["entry"]["tool_call_id"] == "write-proof"
            {
                assert_eq!(1, item["entry"]["turn_number"]);
                results += 1;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(1, results);
    provider.assert_idle();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_are_scoped_cancellable_and_readable_after_server_restart() {
    const MARKER: &str = "Run the background fixture";
    let mock = mock_provider::start_mock_provider().await;
    let mut server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let program = install_gate(server.workspace.path());
    let launch = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains(MARKER)
            .matches(|request| !request_contains_tool_result(request));
        then.status(200).body(tool_response(
            "run_command",
            "job-call",
            json!({
                "program":program,"args":["--ignored","--exact","gated_command_fixture","--nocapture"],
                "cwd":".","timeout":TOOL_TIMEOUT.as_secs(),"brief":"wait for the fixture",
                "background":true,
            }),
        ));
    });
    mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| {
                request_contains_tool_result(request)
                    || !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                        .contains(MARKER)
            });
        then.status(200)
            .body(text_response("Fixture turn settled."));
    });
    let id = "background-job-routes";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"initial","text":"Set up the Session","create":{}}),
        )
        .await;
    server
        .submit(
            id,
            json!({"type":"create_agent","command_id":"create-reviewer","agent":"reviewer"}),
        )
        .await;
    server
        .submit(
            id,
            json!({"type":"send","command_id":"launch","agent":"reviewer","text":MARKER}),
        )
        .await;

    let get = wreq::Method::GET;
    let delete = wreq::Method::DELETE;
    let job = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, response) =
                job_request(&server, get.clone(), id, "jobs", "reviewer").await;
            assert_eq!(200, status, "{response}");
            if !response["jobs"][0].is_null()
                && server.workspace.path().join("tool-attempts.txt").is_file()
            {
                break response["jobs"][0].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the background fixture should start");
    let job_id = job["id"].as_str().unwrap();
    let path = format!("jobs/{job_id}");
    assert_eq!("running", job["status"]);
    let service = server
        .request(get.clone(), "/service/status")
        .send()
        .await
        .unwrap();
    assert_eq!(200, service.status());
    let service: Value = service.json().await.unwrap();
    let busy_job = format!("{id}/reviewer/{job_id} (");
    assert!(service["busy"].as_array().unwrap().iter().any(|work| {
        work.as_str()
            .is_some_and(|label| label.starts_with(&busy_job))
    }));
    assert_eq!(
        (200, job.clone()),
        job_request(&server, get.clone(), id, &path, "reviewer").await
    );
    assert_eq!(
        (200, json!({"jobs":[]})),
        job_request(&server, get.clone(), id, "jobs", "main").await
    );
    for method in [get.clone(), delete.clone()] {
        let (status, response) = job_request(&server, method, id, &path, "main").await;
        assert_eq!(404, status, "{response}");
        assert_eq!("job_not_found", response["code"]);
    }

    let (status, cancelled) = tokio::time::timeout(
        Duration::from_secs(5),
        job_request(&server, delete.clone(), id, &path, "reviewer"),
    )
    .await
    .expect("cancellation should await process cleanup");
    assert_eq!(200, status, "{cancelled}");
    assert_eq!("cancelled", cancelled["status"]);
    assert_eq!(false, cancelled["result"]["cleanup_incomplete"]);
    let cursor = cancelled["finished_at"].as_u64().unwrap();
    let after = (cursor - 1).to_string();
    let mut observer = server
        .session_request(id, "observe", &[("agent", "reviewer"), ("after", &after)])
        .send()
        .await
        .unwrap()
        .bytes_stream();
    let frame = seek_frame(&mut observer, &mut Vec::new(), "Job completion", |frame| {
        frame["id"] == cursor
    })
    .await;
    assert_eq!("job_finished", frame["operation"]);
    let entry = &frame["entry"];
    assert_eq!("job_finished", entry["type"]);
    assert_eq!("reviewer", entry["agent"]);
    assert_eq!(job_id, entry["job_id"]);
    assert_eq!(job["turn"], entry["turn"]);
    assert_eq!(cancelled["status"], entry["status"]);
    assert_eq!(cancelled["output_id"], entry["output_id"]);
    drop(observer);

    server.restart().await;
    let (status, history) = job_request(&server, get.clone(), id, "history", "reviewer").await;
    assert_eq!(200, status, "{history}");
    assert!(history["entries"].as_array().unwrap().contains(entry));
    let (status, history) = job_request(&server, get.clone(), id, "history", "main").await;
    assert_eq!(200, status, "{history}");
    assert!(history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["type"] != "job_finished"));
    let (status, list) = job_request(&server, get.clone(), id, "jobs", "reviewer").await;
    assert_eq!(200, status, "{list}");
    assert_eq!("cancelled", list["jobs"][0]["status"]);
    assert!(list["jobs"][0]["result"].is_null());
    for method in [get, delete] {
        assert_eq!(
            (200, cancelled.clone()),
            job_request(&server, method, id, &path, "reviewer").await
        );
    }
    launch.assert_hits(1);
    server.close().await;
}

async fn job_request(
    server: &TestServer,
    method: wreq::Method,
    id: &str,
    path: &str,
    agent: &str,
) -> (u16, Value) {
    let response = server
        .request(method, &format!("/sessions/{id}/{path}"))
        .query(&[
            ("workspace", server.workspace.path().to_str().unwrap()),
            ("agent", agent),
        ])
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_steer_reaches_the_current_turn_after_a_real_tool_boundary() {
    let mock = mock_provider::start_mock_provider().await;
    let mut server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let program = install_gate(server.workspace.path());
    let first = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| !request_contains_tool_result(request));
        then.status(200)
            .body(mock_provider::anthropic_sse_response(json!({
                "content":[
                    {"type":"text","text":"Running the gate."},
                    {"type":"tool_use","id":"gated-tool","name":"run_command","input":{
                        "program":program,"args":["--ignored","--exact","gated_command_fixture","--nocapture"],
                        "cwd":".","timeout":TOOL_TIMEOUT.as_secs(),"brief":"wait for a file"
                    }}
                ],
                "stop_reason":"tool_use",
                "usage":{
                    "input_tokens":72_000,"output_tokens":6,
                    "cache_read_input_tokens":20_000,"cache_creation_input_tokens":8_000
                }
            })));
    });
    let steer = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| {
                let body = String::from_utf8_lossy(request.body.as_deref().unwrap_or_default());
                body.contains("\"tool_result\"")
                    && body.contains("steering-message")
                    && !body.contains("next-turn-message")
            });
        then.status(200).body(text_response("current turn steered"));
    });
    let next = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("next-turn-message");
        then.status(200).body(text_response("next turn ran"));
    });
    let id = "turn-boundary";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"initial","text":"wait for test signal","create":{}}),
        )
        .await;
    let started = tokio::time::timeout(Duration::from_secs(6), async {
        while !server.workspace.path().join("tool-attempts.txt").is_file() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if started.is_err() {
        let snapshot = server.snapshot(id, "main").await;
        panic!(
            "the command fixture did not reach its file gate: {}",
            snapshot["agent"]
        );
    }
    server.submit(id, json!({"type":"create_agent","command_id":"second-observer","agent":"observer","config":{}})).await;
    let peer = activity_snapshot(&server, id, "observer").await;
    assert!(peer.get("progress").is_none());
    assert_eq!(
        "running_tools",
        agent_summary(&peer, "main")["progress"]["phase"]
    );
    assert_eq!(
        json!(["gated-tool"]),
        agent_summary(&peer, "main")["progress"]["tool_call_ids"]
    );
    let queued = server.submit(id, json!({"type":"send","command_id":"later-turn","text":"next-turn-message","timing":"next_turn"})).await;
    let inserted = server.submit(id, json!({"type":"send","command_id":"steering","text":"steering-message","timing":"steer"})).await;
    assert_eq!("queued", queued["status"]);
    assert_eq!("queued", inserted["status"]);
    let waiting = server.snapshot(id, "main").await;
    assert_eq!(2, waiting["agent"]["pending_inputs"]);
    assert_eq!("running", waiting["agent"]["status"]);
    let tasks = agent_summary(&waiting, "main");
    assert_eq!("wait for test signal", tasks["admitted_task"]);
    assert_eq!("steering-message", tasks["queued_task"]);
    let in_progress = usage(&waiting);
    assert_eq!(Some(100_000), in_progress.input_tokens);
    assert_eq!(Some(6), in_progress.output_tokens);
    assert_eq!(Some(50.0), in_progress.context_percent());
    assert_eq!(Some(20.0), in_progress.cache_hit_percent());
    assert!(in_progress
        .output_tokens_per_second()
        .is_some_and(|rate| rate.is_finite() && rate > 0.0));
    assert!(waiting.get("progress").is_none());
    let progress = server
        .session_request(id, "snapshot", &[("agent", "main"), ("progress", "true")])
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        json!({"phase":"running_tools","tool_call_ids":["gated-tool"]}),
        progress["progress"]
    );
    let mut observer = server
        .session_request(id, "observe", &[("agent", "main"), ("after", "0")])
        .send()
        .await
        .unwrap()
        .bytes_stream();
    let dispatch = tokio::time::timeout(Duration::from_secs(3), async {
        let mut buffer = Vec::new();
        loop {
            let item = common::stream::next_json_line(&mut observer, &mut buffer, "dispatch").await;
            if item["kind"] == "runtime.tool_dispatched" {
                break item;
            }
        }
    })
    .await
    .expect("the running tool dispatch was committed");
    assert_eq!("gated-tool", dispatch["tool_call_id"]);
    drop(observer);
    assert!(
        queued["receipt"]["event_id"].as_u64().unwrap()
            < inserted["receipt"]["event_id"].as_u64().unwrap()
    );
    fs::write(server.workspace.path().join("release-tool"), b"run").unwrap();
    let completed = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "next turn ran"
        })
        .await;
    let tasks = agent_summary(&completed, "main");
    assert_eq!("next-turn-message", tasks["admitted_task"]);
    assert!(tasks["queued_task"].is_null());
    let cumulative = usage(&completed);
    assert_eq!(Some(100_016), cumulative.input_tokens);
    assert_eq!(Some(14), cumulative.output_tokens);
    assert_eq!(3, cumulative.model_requests);
    let turns = EventStore::replay(server.events_path(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| {
            if let EventPayload::ModelResponse { turn, .. } = event.payload {
                Some(turn)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(3, turns.len());
    assert_eq!(
        turns[0], turns[1],
        "the steering message must stay in the same Turn"
    );
    assert!(
        turns[2] > turns[1],
        "the queued message must use a later Turn"
    );
    first.assert_hits(1);
    steer.assert_hits(1);
    next.assert_hits(1);

    server.restart().await;
    assert_eq!(cumulative, usage(&server.snapshot(id, "main").await));
    let second_page: Value = server
        .session_request(id, "history", &[("agent", "main"), ("limit", "1")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(turns[2], second_page["entries"][0]["turn"]);
    assert_eq!(2, second_page["entries"][0]["turn_number"]);
    let first_page: Value = server
        .session_request(
            id,
            "history",
            &[
                ("agent", "main"),
                ("limit", "1"),
                ("before", &turns[2].to_string()),
            ],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(turns[0], first_page["entries"][0]["turn"]);
    assert_eq!(1, first_page["entries"][0]["turn_number"]);
    server
        .submit(
            id,
            json!({"type":"create_agent","agent":"reviewer","command_id":"reviewer","config":{}}),
        )
        .await;
    let reviewer = usage(&server.snapshot(id, "reviewer").await);
    assert_eq!(None, reviewer.input_tokens);
    assert_eq!(None, reviewer.output_tokens);
    assert_eq!(None, reviewer.context_percent());
    assert_eq!(None, reviewer.cache_hit_percent());
    assert_eq!(None, reviewer.output_tokens_per_second());

    let history: Value = server
        .session_request(id, "history", &[("limit", "256")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entry = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["type"] == "tool_result" && entry["tool_call_id"] == "gated-tool")
        .unwrap();
    assert_eq!(0, entry["structured"]["exit_code"]);
    let reference = entry["output_id"].as_str().unwrap();
    let first_bytes: Value = server
        .session_request(id, &format!("outputs/{reference}"), &[("max_bytes", "3")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(3, first_bytes["output"]["next_offset"]);
    assert_eq!(true, first_bytes["output"]["has_more"]);
    let more: Value = server
        .session_request(id, &format!("outputs/{reference}"), &[("offset", "3")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(3, more["output"]["offset"]);
    assert_eq!(false, more["output"]["has_more"]);
}

fn usage(snapshot: &Value) -> kuku::UsageTotals {
    serde_json::from_value(snapshot["agent"]["usage"].clone()).unwrap()
}

#[tokio::test]
async fn a_connected_observer_does_not_retain_an_idle_writer() {
    let mock = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&mock, "idle with observer");
    let config = mock_provider::make_test_config(mock.port());
    let server = TestServer::start(config.clone()).await;
    let id = "idle-monitor";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"first","text":"settle and release","create":{}}),
        )
        .await;
    let observer = server
        .session_request(id, "observe", &[])
        .send()
        .await
        .unwrap();
    assert_eq!(200, observer.status());
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "idle with observer"
        })
        .await;
    let mut options =
        kuku::SessionOptions::new(server.workspace.path(), server.home.path(), config);
    options.session_id = Some(id.into());
    let reopened = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            match kuku::Session::open(options.clone()).await {
                Ok(session) => break session,
                Err(kuku::Error::SessionLocked { .. }) => {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(error) => panic!("unexpected reopen error: {error}"),
            }
        }
    })
    .await
    .expect("the host must release the idle writer even while an observer is connected");
    reopened.close().await.unwrap();
    drop(observer);
}

#[tokio::test]
async fn multiple_projects_share_one_host_and_each_agent_resolves_its_named_connection() {
    let first = mock_provider::start_mock_provider().await;
    let second = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&first, "first result");
    let selected = second.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("second material")
            .matches(|request| {
                !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                    .contains("first material")
            });
        then.status(200).body(text_response("second result"));
    });
    let mut config = mock_provider::make_test_config(first.port());
    let mut connection = config.providers["anthropic"].clone();
    connection.base_url = second.base_url();
    config.providers.insert("secondary".into(), connection);
    let server = TestServer::start(config).await;
    let workspace = tempfile::tempdir().unwrap();
    let id = "shared-service";
    server.submit(id, json!({"type":"send","command_id":"same","text":"first material","create":{"connection":"anthropic","model":"fixture-model"}})).await;
    server.submit(id, json!({"type":"send","command_id":"same","text":"second material","workspace":workspace.path(),
        "create":{"connection":"secondary","model":"fixture-model"}})).await;

    let snapshot = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "first result"
        })
        .await;
    assert_eq!("anthropic", snapshot["selection"]["connection"]);
    let second_workspace = workspace.path().canonicalize().unwrap();
    let seen = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response: Value = server
                .request(wreq::Method::GET, &format!("/sessions/{id}/snapshot"))
                .query(&[("workspace", second_workspace.to_str().unwrap())])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if response["agent"]["last_output"] == "second result" {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!("secondary", seen["selection"]["connection"]);
    assert_eq!(json!(second_workspace), seen["workspace"]);
    selected.assert_hits(1);
}

fn request_contains_tool_result(request: &httpmock::prelude::HttpMockRequest) -> bool {
    String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("\"tool_result\"")
}

fn gate_filename() -> String {
    format!("tool-gate{}", env::consts::EXE_SUFFIX)
}

fn install_gate(workspace: &Path) -> PathBuf {
    let executable = env::current_exe().unwrap();
    let filename = gate_filename();
    let installed = workspace.join(&filename);
    if fs::hard_link(&executable, &installed).is_err() {
        fs::copy(&executable, installed).unwrap();
    }
    Path::new(".").join(filename)
}

#[test]
#[ignore = "subprocess fixture invoked by the tool boundary test"]
fn gated_command_fixture() {
    if Path::new(&gate_filename()).is_file() {
        kuku::test_support::run_gated_tool(TOOL_TIMEOUT).unwrap();
    }
}
