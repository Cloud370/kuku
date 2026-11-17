mod common;

use std::env;
use std::fs;
use std::path::Path;
use std::time::Duration;

use common::mock_provider::{self, text_response, tool_response};
use common::TestServer;
use httpmock::Method;
use kuku::event::{EventPayload, EventStore};
use serde_json::{json, Value};

const TOOL_TIMEOUT: Duration = Duration::from_secs(20);

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
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let command = install_gate(server.workspace.path());
    let first = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| !request_contains_tool_result(request));
        then.status(200).body(tool_response(
            "run_command",
            "gated-tool",
            json!({
                "command":command,"timeout":TOOL_TIMEOUT.as_secs(),"brief":"wait for a file"
            }),
        ));
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
    assert!(
        queued["receipt"]["event_id"].as_u64().unwrap()
            < inserted["receipt"]["event_id"].as_u64().unwrap()
    );
    fs::write(server.workspace.path().join("release-tool"), b"run").unwrap();
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "next turn ran"
        })
        .await;
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
