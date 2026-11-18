mod common;

use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::routing::post;
use axum::Router;
use common::mock_provider::{self, text_response, tool_response};
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
    let (port, begin, finish, provider) = paused_peer_reply(&text).await;
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
    let mut parent = observe_agent_activity(&server, id, "main", cursor)
        .await
        .bytes_stream();
    let mut reviewer = observe_agent_activity(&server, id, "reviewer", cursor)
        .await
        .bytes_stream();
    let mut parent_buffer = Vec::new();
    let mut reviewer_buffer = Vec::new();

    server.submit(id, json!({"type":"send","command_id":"peer-input","agent":"reviewer","text":"hold peer stream"})).await;
    let step =
        |frame: &Value| frame["agent"] == "reviewer" && frame["kind"] == "runtime.step_prepared";
    seek_frame(&mut parent, &mut parent_buffer, step).await;
    seek_frame(&mut reviewer, &mut reviewer_buffer, step).await;
    let running = server.snapshot(id, "main").await;
    let peer = agent_summary(&running, "reviewer");
    assert_eq!("running", peer["status"]);
    assert!(peer["last_output_preview"].is_null());
    begin.send(()).unwrap();

    let text_frame =
        |frame: &Value| frame["agent"] == "reviewer" && frame["event"]["type"] == "text";
    let related = seek_frame(&mut parent, &mut parent_buffer, text_frame).await;
    let selected = seek_frame(&mut reviewer, &mut reviewer_buffer, text_frame).await;
    assert_eq!(peer["execution_id"], related["execution_id"]);
    assert_eq!(text.as_str(), selected["event"]["content"]);
    let preview = &text[..text.floor_char_boundary(512)];
    assert_eq!(preview, related["event"]["content"]);
    finish.send(()).unwrap();

    let end = seek_frame(&mut parent, &mut parent_buffer, |frame| {
        frame["agent"] == "reviewer" && frame["operation"] == "turn_settled"
    })
    .await;
    assert_eq!("idle", end["settlement"]["status"]);
    assert_eq!(preview, end["settlement"]["output"]);
    assert!(end["entry"].is_null());
    drop(parent);
    drop(reviewer);
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
) -> (
    u16,
    oneshot::Sender<()>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let response = text_response(text);
    let split = response.find("event: content_block_stop").unwrap();
    let (begin, wait_for_begin) = oneshot::channel();
    let (finish, wait_for_finish) = oneshot::channel();
    let pending = Arc::new(Mutex::new(Some((
        response[..split].to_owned(),
        response[split..].to_owned(),
        wait_for_begin,
        wait_for_finish,
    ))));
    let app = Router::new().route(
        "/v1/messages",
        post(move |body: String| {
            let pending = pending.clone();
            async move {
                let body = if body.contains("hold peer stream") {
                    let (first, rest, begin, finish) = pending.lock().await.take().unwrap();
                    let (send, read) = mpsc::channel(1);
                    tokio::spawn(async move {
                        if begin.await.is_ok() {
                            let _ = send
                                .send(Ok::<_, std::convert::Infallible>(Bytes::from(first)))
                                .await;
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
    (port, begin, finish, handle)
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
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = common::stream::next_json_line(stream, buffer, "peer observation").await;
            if predicate(&frame) {
                return frame;
            }
        }
    })
    .await
    .expect("the peer observation should reach the requested boundary")
}

fn agent_summary<'a>(snapshot: &'a Value, address: &str) -> &'a Value {
    snapshot["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["address"] == address)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observer_disconnect_leaves_exactly_one_durable_tool_result() {
    let mock = mock_provider::start_mock_provider().await;
    let call = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| !request_contains_tool_result(request));
        then.status(200)
            .delay(Duration::from_millis(300))
            .body(tool_response(
                "write_file",
                "write-proof",
                json!({
                    "path":"proof.txt","content":"persisted\n","brief":"write proof"
                }),
            ));
    });
    let continuation = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(request_contains_tool_result)
            .body_contains("write-proof");
        then.status(200).body(text_response("Proof committed."));
    });
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "durable-result";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"command","text":"write proof","create":{}}),
        )
        .await;
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
                results += 1;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(1, results);
    call.assert_hits(1);
    continuation.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_steer_reaches_the_current_turn_after_a_real_tool_boundary() {
    let mock = mock_provider::start_mock_provider().await;
    let mut server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let command = install_gate(server.workspace.path());
    let first = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| !request_contains_tool_result(request));
        then.status(200)
            .body(mock_provider::anthropic_sse_response(json!({
                "content":[
                    {"type":"text","text":"Running the gate."},
                    {"type":"tool_use","id":"gated-tool","name":"run_command","input":{
                        "command":command,"timeout":TOOL_TIMEOUT.as_secs(),"brief":"wait for a file"
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
    let queued = server.submit(id, json!({"type":"send","command_id":"later-turn","text":"next-turn-message","timing":"next_turn"})).await;
    let inserted = server.submit(id, json!({"type":"send","command_id":"steering","text":"steering-message","timing":"steer"})).await;
    assert_eq!("queued", queued["status"]);
    assert_eq!("queued", inserted["status"]);
    let waiting = server.snapshot(id, "main").await;
    assert_eq!(2, waiting["agent"]["pending_inputs"]);
    assert_eq!("running", waiting["agent"]["status"]);
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

fn install_gate(workspace: &Path) -> String {
    let executable = env::current_exe().unwrap();
    let filename = gate_filename();
    let installed = workspace.join(&filename);
    if fs::hard_link(&executable, &installed).is_err() {
        fs::copy(&executable, installed).unwrap();
    }
    let local = Path::new(".").join(filename);
    format!(
        "{} --ignored --exact gated_command_fixture --nocapture",
        local.display()
    )
}

#[test]
#[ignore = "subprocess fixture invoked by the tool boundary test"]
fn gated_command_fixture() {
    if Path::new(&gate_filename()).is_file() {
        kuku::test_support::run_gated_tool(TOOL_TIMEOUT).unwrap();
    }
}
