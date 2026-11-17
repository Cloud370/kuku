use std::path::Path;
use std::process::{Command, Output};

use httpmock::{Method, MockServer};
use kuku::session::{AgentStatus, Session};
use serde_json::{json, Value};

fn tool_round(request: &httpmock::prelude::HttpMockRequest) -> usize {
    let body: Value =
        serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap_or(Value::Null);
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let has_results = |message: &Value| {
        message["role"] == "user"
            && message["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
    };
    let count = messages
        .iter()
        .filter(|message| has_results(message))
        .count();
    if count > 0 && !messages.last().is_some_and(has_results) {
        usize::MAX
    } else {
        count
    }
}

fn mock_round(server: &MockServer, round: usize, content: Value, stop_reason: &str) {
    mock_round_delayed(
        server,
        round,
        content,
        stop_reason,
        std::time::Duration::ZERO,
    );
}

fn mock_round_delayed(
    server: &MockServer,
    round: usize,
    content: Value,
    stop_reason: &str,
    delay: std::time::Duration,
) {
    let matcher: fn(&httpmock::prelude::HttpMockRequest) -> bool = match round {
        0 => |request| tool_round(request) == 0,
        1 => |request| tool_round(request) == 1,
        2 => |request| tool_round(request) == 2,
        3 => |request| tool_round(request) == 3,
        _ => panic!("unsupported fixture round"),
    };
    server.mock(move |when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(matcher);
        then.status(200)
            .delay(delay)
            .header("content-type", "text/event-stream")
            .body(kuku::test_support::anthropic_sse_response(json!({
                "id": format!("msg_{round}"), "type":"message", "role":"assistant",
                "content":content, "stop_reason":stop_reason,
                "usage":{"input_tokens":10,"output_tokens":5},
            })));
    });
}

fn config_file(home: &Path, primary: u16, secondary: u16) -> std::path::PathBuf {
    let path = home.join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"
default_model = "balanced"
[model.balanced]
provider = "primary"
model = "fixture-model"
think = "off"
max_output_tokens = 512
[model.strong]
provider = "primary"
model = "fixture-model"
think = "off"
max_output_tokens = 512
[model.light]
provider = "primary"
model = "fixture-model"
think = "off"
max_output_tokens = 512
[provider.primary]
format = "anthropic"
base_url = "http://127.0.0.1:{primary}"
api_key = "local-fixture"
profile = "compatible"
reasoning_efforts = ["none"]
[provider.secondary]
format = "anthropic"
base_url = "http://127.0.0.1:{secondary}"
api_key = "local-fixture"
profile = "compatible"
reasoning_efforts = ["none"]
"#
        ),
    )
    .unwrap();
    path
}

fn invoke(workspace: &Path, home: &Path, config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .current_dir(workspace)
        .env("KUKU_HOME", home)
        .arg("run")
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .unwrap()
}

fn result(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid result: {error}; {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_persists_file_and_command_results_then_reopens_same_agent() {
    let mock = MockServer::start_async().await;
    mock_round(
        &mock,
        0,
        json!([{"type":"tool_use","id":"read","name":"read_file","input":{"path":"answer.txt"}}]),
        "tool_use",
    );
    mock_round(
        &mock,
        1,
        json!([{"type":"tool_use","id":"write","name":"write_file","input":{"path":"answer.txt","content":"after\n","brief":"update answer"}}]),
        "tool_use",
    );
    mock_round(
        &mock,
        2,
        json!([{"type":"tool_use","id":"verify","name":"run_command","input":{"command":"git diff --no-index --exit-code expected.txt answer.txt","timeout":10,"brief":"verify actual file"}}]),
        "tool_use",
    );
    mock_round(
        &mock,
        3,
        json!([{"type":"text","text":"Verified on disk."}]),
        "end_turn",
    );
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("answer.txt"), "before\n").unwrap();
    std::fs::write(workspace.path().join("expected.txt"), "after\n").unwrap();
    let config = config_file(home.path(), mock.port(), mock.port());
    let output = invoke(
        workspace.path(),
        home.path(),
        &config,
        &[
            "--connection",
            "primary",
            "--model",
            "fixture-model",
            "--json",
            "fix answer and verify",
        ],
    );
    let value = result(&output);
    assert_eq!("completed", value["event"]);
    assert_eq!(
        "after\n",
        std::fs::read_to_string(workspace.path().join("answer.txt")).unwrap()
    );
    assert!(!workspace.path().join(".git").exists());
    let session_id = value["session_id"].as_str().unwrap();
    let path =
        kuku::session::session_events_path(home.path(), workspace.path(), session_id).unwrap();
    let agents = Session::read_agents(&path).unwrap();
    assert_eq!(AgentStatus::Idle, agents[0].status);
    assert_eq!(Some("Verified on disk."), agents[0].last_output.as_deref());
    let events = kuku::event::EventStore::replay(&path).unwrap();
    assert!(events.iter().any(|event| matches!(&event.payload, kuku::event::EventPayload::ToolResult { tool_call_id, status, .. } if tool_call_id == "verify" && status == "ok")));
    let continued = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages").body_contains("Verified on disk.").body_contains("continue inspection");
        then.status(200).body(kuku::test_support::anthropic_sse_response(json!({"id":"continue","type":"message","role":"assistant","content":[{"type":"text","text":"Same context."}],"stop_reason":"end_turn","usage":{"input_tokens":20,"output_tokens":4}})));
    });
    let next = result(&invoke(
        workspace.path(),
        home.path(),
        &config,
        &["--session", session_id, "--json", "continue inspection"],
    ));
    assert_eq!("Same context.", next["response"]);
    assert_eq!("primary", next["config"]["connection"]);
    continued.assert_hits(1);
    let show = Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .current_dir(workspace.path())
        .env("KUKU_HOME", home.path())
        .args(["show", session_id])
        .output()
        .unwrap();
    assert!(show.status.success());
    assert_eq!(
        "Same context.",
        String::from_utf8(show.stdout).unwrap().trim()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_selects_exact_connection_when_protocol_matches() {
    let primary = MockServer::start_async().await;
    let secondary = MockServer::start_async().await;
    mock_round(
        &secondary,
        0,
        json!([{"type":"text","text":"Secondary route."}]),
        "end_turn",
    );
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = config_file(home.path(), primary.port(), secondary.port());
    let output = result(&invoke(
        workspace.path(),
        home.path(),
        &config,
        &[
            "--connection",
            "secondary",
            "--model",
            "fixture-model",
            "--json",
            "choose named route",
        ],
    ));
    assert_eq!("Secondary route.", output["response"]);
    assert_eq!("secondary", output["config"]["connection"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_reopens_question_and_rejects_stale_answers_without_new_work() {
    let mock = MockServer::start_async().await;
    let question = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("color=blue")
        });
        then.status(200).body(kuku::test_support::anthropic_sse_response(json!({
            "id":"ask","type":"message","role":"assistant","content":[{"type":"tool_use","id":"ask-color","name":"ask_user","input":{"question":"Which color?"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let answer = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages").body_contains("color=blue");
        then.status(200).body(kuku::test_support::anthropic_sse_response(json!({
            "id":"answer","type":"message","role":"assistant","content":[{"type":"text","text":"Blue recorded."}],
            "stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":4}
        })));
    });
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = config_file(home.path(), mock.port(), mock.port());
    let waiting = result(&invoke(
        workspace.path(),
        home.path(),
        &config,
        &["--command-id", "color-input", "--json", "ask for a color"],
    ));
    assert_eq!("waiting", waiting["event"]);
    let session_id = waiting["session_id"].as_str().unwrap();
    let request_id = waiting["question"]["id"].as_str().unwrap();
    let duplicate = result(&invoke(
        workspace.path(),
        home.path(),
        &config,
        &[
            "--session",
            session_id,
            "--command-id",
            "color-input",
            "--json",
            "ask for a color",
        ],
    ));
    assert_eq!("acknowledged", duplicate["event"]);
    assert_eq!(true, duplicate["replayed"]);
    assert_eq!(
        waiting["question"]["id"],
        duplicate["state"]["question"]["id"]
    );
    assert_eq!("waiting", duplicate["state"]["status"]);

    let reply = |command_id: &str| {
        Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
            .current_dir(workspace.path())
            .env("KUKU_HOME", home.path())
            .args(["reply", session_id, request_id, "--config"])
            .arg(&config)
            .args(["--answer", "color=blue", "--command-id", command_id])
            .output()
            .unwrap()
    };
    let first = result(&reply("reply-color"));
    assert_eq!("Blue recorded.", first["last_output"]);
    assert_eq!("idle", first["status"]);
    let retry = result(&reply("reply-color"));
    assert_eq!("acknowledged", retry["event"]);
    assert_eq!(first["usage"], retry["state"]["usage"]);
    assert!(!reply("stale-new-command").status.success());
    question.assert_hits(1);
    answer.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_main_turn_completion_waits_for_running_independent_agent() {
    let primary = MockServer::start_async().await;
    let secondary = MockServer::start_async().await;
    mock_round(
        &primary,
        0,
        json!([
            {"type":"tool_use","id":"create-worker","name":"create_agent","input":{"address":"worker","connection":"secondary","model":"fixture-model","instructions":"Write the delegated proof file."}},
            {"type":"tool_use","id":"send-worker","name":"send_message","input":{"to":"worker","message":"write delegated-proof.txt with delegated\n"}}
        ]),
        "tool_use",
    );
    mock_round(
        &primary,
        1,
        json!([{"type":"text","text":"Delegated."}]),
        "end_turn",
    );
    mock_round_delayed(
        &secondary,
        0,
        json!([
            {"type":"tool_use","id":"write-delegated","name":"write_file","input":{"path":"delegated-proof.txt","content":"delegated\n","brief":"write delegated proof"}}
        ]),
        "tool_use",
        std::time::Duration::from_millis(150),
    );
    mock_round(
        &secondary,
        1,
        json!([{"type":"text","text":"Worker completed."}]),
        "end_turn",
    );
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = config_file(home.path(), primary.port(), secondary.port());
    let completed = result(&invoke(
        workspace.path(),
        home.path(),
        &config,
        &["--json", "delegate file creation"],
    ));
    assert_eq!("Delegated.", completed["response"]);
    assert_eq!(
        "delegated\n",
        std::fs::read_to_string(workspace.path().join("delegated-proof.txt")).unwrap()
    );
    let path = kuku::session::session_events_path(
        home.path(),
        workspace.path(),
        completed["session_id"].as_str().unwrap(),
    )
    .unwrap();
    let worker = Session::read_agents(&path)
        .unwrap()
        .into_iter()
        .find(|agent| agent.address == "worker")
        .unwrap();
    assert_eq!(AgentStatus::Idle, worker.status);
    assert_eq!(Some("Worker completed."), worker.last_output.as_deref());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_repeated_command_acknowledges_interruption_without_authorizing_resume() {
    let mock = MockServer::start_async().await;
    let model = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages");
        then.status(200).delay(std::time::Duration::from_secs(3)).body(kuku::test_support::anthropic_sse_response(json!({
            "id":"interrupted","type":"message","role":"assistant","content":[{"type":"text","text":"Must not resume on retry."}],
            "stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config_path = config_file(home.path(), mock.port(), mock.port());
    let config = kuku::config::load_config(&config_path)
        .unwrap()
        .resolve()
        .unwrap();
    let mut options = kuku::SessionOptions::new(workspace.path(), home.path(), config);
    options.session_id = Some("retry-interrupted".into());
    let seed = Session::open(options.clone()).await.unwrap();
    seed.create_agent("create-main", "main", kuku::AgentConfig::default())
        .unwrap();
    seed.close().await.unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .current_dir(workspace.path())
        .env("KUKU_HOME", home.path())
        .args(["run", "--config"])
        .arg(&config_path)
        .args([
            "--session",
            "retry-interrupted",
            "--command-id",
            "interrupted-input",
            "--json",
            "original interrupted work",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while model.hits_async().await == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("CLI should durably accept input before beginning its model request");
    child.kill().unwrap();
    child.wait().unwrap();
    let acknowledged = result(&invoke(
        workspace.path(),
        home.path(),
        &config_path,
        &[
            "--session",
            "retry-interrupted",
            "--command-id",
            "interrupted-input",
            "--json",
            "original interrupted work",
        ],
    ));
    assert_eq!("acknowledged", acknowledged["event"]);
    assert_eq!("interrupted", acknowledged["state"]["status"]);
    let restored = Session::open(options).await.unwrap();
    assert_eq!(
        AgentStatus::Interrupted,
        restored.agent("main").unwrap().status
    );
    let mut cursor = 0;
    loop {
        let page = restored.events(cursor, 128).unwrap();
        if page.is_empty() {
            break;
        }
        for event in page {
            cursor = event.id;
            if let kuku::event::EventPayload::Unknown(value) = event.payload {
                assert_ne!("agent_resumed", value["record"]["operation"]);
            }
        }
    }
    restored.close().await.unwrap();
    model.assert_hits(1);
    let continued = result(&invoke(
        workspace.path(),
        home.path(),
        &config_path,
        &[
            "--session",
            "retry-interrupted",
            "--command-id",
            "authorized-continuation",
            "--json",
            "explicitly continue after checking no effects",
        ],
    ));
    assert_eq!("completed", continued["event"]);
    assert_eq!("idle", continued["status"]);
    model.assert_hits(2);
}
