use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use httpmock::{Method, MockServer};
use kuku::session::AgentStatus;
use kuku_client::local::LocalService;
use serde_json::{Value, json};

use super::{config_file, invoke, json_lines, mock_round, result, run_cli, stop_managed};

fn request_has_scoped_text(request: &httpmock::prelude::HttpMockRequest, text: &str) -> bool {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
    let tools = body["tools"].as_array().unwrap();
    tools.len() == 1
        && tools[0]["name"] == "read_file"
        && body["messages"].as_array().unwrap().iter().any(|message| {
            message["role"] == "user"
                && message["content"].as_array().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block["type"] == "text" && block["text"] == text)
                })
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_tty_file_and_stdin_send_the_same_prompt_and_resume_the_session() {
    const FIRST: &str = "/literal not a skill\n\n  第二行  \n";
    const SECOND: &str = "Continue after the multiline prompt.\n";
    let mock = MockServer::start_async().await;
    let first_request = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| request_has_scoped_text(request, FIRST) && !request_has_scoped_text(request, SECOND));
        then.status(200)
            .body(kuku::test_support::anthropic_sse_response(json!({
                "id":"multiline","type":"message","role":"assistant","content":[{"type":"text","text":"First reply."}],
                "stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":4}
            })));
    });
    let continued_request = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| request_has_scoped_text(request, FIRST) && request_has_scoped_text(request, SECOND));
        then.status(200)
            .body(kuku::test_support::anthropic_sse_response(json!({
                "id":"continue","type":"message","role":"assistant","content":[{"type":"text","text":"Second reply."}],
                "stop_reason":"end_turn","usage":{"input_tokens":9,"output_tokens":4}
            })));
    });
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let patch = format!(
        r#"
default_model = {{ connection = "fixture", model = "model.for.multiline" }}
[provider.fixture]
format = "anthropic"
base_url = "{}"
api_key = "fixture-key"
[model.fixture."model.for.multiline"]
max_output_tokens = 512
think = "off"
"#,
        mock.base_url()
    );
    let created = Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .env("KUKU_HOME", home.path())
        .args(["config", "set", "--patch", &patch])
        .output()
        .unwrap();
    assert_eq!("updated", result(&created)["status"]);

    let agents = workspace.path().join(".kuku").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("fixture-files.md"),
        "---\nname: fixture-files\ndescription: Local read-only fixture\ntools: [read_file]\n---\nRead the fixture file when needed.\n",
    ).unwrap();
    let prompt_file = workspace.path().join("prompt.txt");
    std::fs::write(&prompt_file, FIRST).unwrap();
    let file = run_cli(workspace.path(), home.path(), &config)
        .args([
            "--json",
            "--command-id",
            "first-file-input",
            "--preset",
            "fixture-files",
            "--prompt-file",
        ])
        .arg(&prompt_file)
        .output()
        .unwrap();
    let file_result = result(&file);
    assert_eq!("completed", file_result["status"]);
    assert_eq!("First reply.", file_result["response"]);
    assert_eq!("model.for.multiline", file_result["model"]);
    assert_eq!("fixture-files", file_result["config"]["preset"]);

    let mut child = run_cli(workspace.path(), home.path(), &config)
        .args([
            "--stream-json",
            "--preset",
            "fixture-files",
            "--prompt-file",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(FIRST.as_bytes())
        .unwrap();
    let piped = child.wait_with_output().unwrap();
    assert!(
        piped.status.success(),
        "{}",
        String::from_utf8_lossy(&piped.stderr)
    );
    let records = json_lines(&piped);
    assert_eq!("started", records[0]["event"]);
    assert_eq!("completed", records.last().unwrap()["status"]);
    assert_eq!(file_result["response"], records.last().unwrap()["response"]);
    first_request.assert_hits(2);

    let session_id = file_result["session_id"].as_str().unwrap();
    std::fs::write(&prompt_file, SECOND).unwrap();
    let resumed = run_cli(workspace.path(), home.path(), &config)
        .args(["--json", "--session", session_id, "--prompt-file"])
        .arg(&prompt_file)
        .output()
        .unwrap();
    let resumed = result(&resumed);
    assert_eq!(session_id, resumed["session_id"]);
    assert_eq!("Second reply.", resumed["response"]);

    std::fs::write(&prompt_file, FIRST).unwrap();
    let replayed = run_cli(workspace.path(), home.path(), &config)
        .args([
            "--stream-json",
            "--session",
            session_id,
            "--command-id",
            "first-file-input",
            "--prompt-file",
        ])
        .arg(&prompt_file)
        .output()
        .unwrap();
    let receipt = result(&replayed);
    assert_eq!("acknowledged", receipt["event"]);
    assert_eq!("acknowledged", receipt["status"]);
    assert_eq!(file_result["receipt"], receipt["receipt"]);
    assert_eq!(true, receipt["replayed"]);
    assert!(receipt["response"].is_null());
    first_request.assert_hits(2);
    continued_request.assert_hits(1);
    stop_managed(home.path()).await;

    let path =
        kuku::session::session_events_path(home.path(), workspace.path(), session_id).unwrap();
    let events = kuku::event::EventStore::replay(&path).unwrap();
    let inputs: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            kuku::event::EventPayload::MessageUser { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!([FIRST, SECOND], inputs.as_slice());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_execution_ends_no_tty_stream_with_cancelled_status() {
    let mock = MockServer::start_async().await;
    let request = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages");
        then.status(200)
            .delay(Duration::from_secs(10))
            .body(kuku::test_support::anthropic_sse_response(json!({
                "id":"cancel","type":"message","role":"assistant","content":[{"type":"text","text":"Too late."}],
                "stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":4}
            })));
    });
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = config_file(home.path(), mock.port(), mock.port());
    let mut child = run_cli(workspace.path(), home.path(), &config)
        .args(["--stream-json", "run until cancelled"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut started = String::new();
    stdout.read_line(&mut started).unwrap();
    let started_record: Value = serde_json::from_str(&started).unwrap();
    assert_eq!("started", started_record["event"]);
    let target = kuku_client::Target {
        workspace: workspace.path().canonicalize().unwrap(),
        session_id: started_record["session_id"].as_str().unwrap().into(),
        agent: "main".into(),
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while request.hits_async().await == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("provider must receive a request before cancellation");
    let service = LocalService::resolve(home.path(), None).unwrap();
    let client = kuku_client::Client::local(&service).await.unwrap();
    let execution_id = client
        .snapshot(&target)
        .await
        .unwrap()
        .execution_id()
        .unwrap();
    client
        .submit(&kuku_client::Command {
            target: target.clone(),
            command_id: "fixture-cancel".into(),
            action: kuku_client::Action::Cancel { execution_id },
        })
        .await
        .unwrap();

    let mut output = child.wait_with_output().unwrap();
    output.stdout = started.into_bytes();
    stdout.read_to_end(&mut output.stdout).unwrap();
    assert_eq!(
        Some(130),
        output.status.code(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = json_lines(&output);
    let result = records.last().unwrap();
    assert_eq!("cancelled", result["event"]);
    assert_eq!("cancelled", result["status"]);
    assert_eq!("stopped", result["agent_status"]);
    assert_eq!(target.session_id, result["session_id"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("cancelled"));
    stop_managed(home.path()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_config_commands_follow_the_active_file_unless_explicitly_overridden() {
    let mock = MockServer::start_async().await;
    mock_round(
        &mock,
        0,
        json!([{"type":"text","text":"Service is ready."}]),
        "end_turn",
    );
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let fallback = config_file(home.path(), mock.port(), mock.port());
    let custom = workspace.path().join("custom.toml");
    let custom_bytes = std::fs::read_to_string(&fallback).unwrap();
    std::fs::write(&custom, &custom_bytes).unwrap();
    let fallback_bytes = custom_bytes.replace("fixture-model", "data-dir-model");
    std::fs::write(&fallback, &fallback_bytes).unwrap();
    let run = invoke(
        workspace.path(),
        home.path(),
        &custom,
        &["--json", "start fixture service"],
    );
    assert_eq!("completed", result(&run)["status"]);
    let custom = custom.canonicalize().unwrap();
    let fallback = fallback.canonicalize().unwrap();

    let ask_config = |args: &[&str], explicit: Option<&Path>| {
        let mut cli = Command::new(env!("CARGO_BIN_EXE_kuku-cli"));
        cli.current_dir(workspace.path())
            .env("KUKU_HOME", home.path());
        if let Some(path) = explicit {
            cli.arg("--config").arg(path);
        }
        result(&cli.arg("config").args(args).output().unwrap())
    };
    for args in [vec!["show", "--json"], vec!["models"], vec!["validate"]] {
        let state = ask_config(&args, None);
        assert_eq!(custom.to_string_lossy().as_ref(), state["path"]);
        assert_eq!("loaded", state["service"]["state"]);
    }
    let set = ask_config(
        &[
            "set",
            "--patch",
            "default_model = { connection = \"primary\", model = \"service.model.changed\" }\n[model.primary.\"service.model.changed\"]\nmax_output_tokens = 512",
        ],
        None,
    );
    assert_eq!(custom.to_string_lossy().as_ref(), set["path"]);
    assert_eq!("updated", set["status"]);
    assert!(matches!(
        set["service"]["state"].as_str(),
        Some("pending_reload" | "loaded")
    ));
    assert_eq!(fallback_bytes, std::fs::read_to_string(&fallback).unwrap());
    assert_ne!(custom_bytes, std::fs::read_to_string(&custom).unwrap());

    let updated_custom = std::fs::read(&custom).unwrap();
    let explicit = ask_config(
        &[
            "set",
            "--patch",
            "default_model = { connection = \"primary\", model = \"selected.directly\" }\n[model.primary.\"selected.directly\"]\nmax_output_tokens = 512",
        ],
        Some(&fallback),
    );
    assert_eq!(fallback.to_string_lossy().as_ref(), explicit["path"]);
    assert_eq!("different_path", explicit["service"]["state"]);
    assert_eq!(updated_custom, std::fs::read(&custom).unwrap());
    assert_ne!(fallback_bytes, std::fs::read_to_string(&fallback).unwrap());

    let service = LocalService::resolve(home.path(), None).unwrap();
    let client = kuku_client::Client::local(&service).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let validated = ask_config(&["validate"], None);
            if validated["service"]["state"] == "loaded" {
                assert_eq!("service.model.changed", validated["default_model"]["model"]);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("service should load the exact file revision");
    let models = client.models().await.unwrap();
    assert_eq!("service.model.changed", models.default_model.model);
    stop_managed(home.path()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn patch_preserves_the_running_services_default_when_the_editors_environment_differs() {
    const VARIABLE: &str = "KUKU_FIXTURE_PREVIOUS_MODEL";
    let mock = MockServer::start_async().await;
    let reply = |model: &str| {
        mock.mock(|when, then| {
            when.method(Method::POST)
                .path("/v1/chat/completions")
                .json_body_partial(json!({"model":model}).to_string());
            then.status(200).header("content-type", "text/event-stream").body(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            );
        })
    };
    let old_requests = reply("model-A");
    let new_requests = reply("model-B");
    for editor_model in [Some("model-B"), None] {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                r#"default_model = {{ connection = "work", model = "${VARIABLE}" }}
[provider.work]
format = "openai-chat"
base_url = "{}"
api_key = "fixture"
"#,
                mock.url("/v1"),
            ),
        )
        .unwrap();
        let first = run_cli(workspace.path(), home.path(), &config)
            .env(VARIABLE, "model-A")
            .args(["--json", "first fixture request"])
            .output()
            .unwrap();
        let first = result(&first);
        assert_eq!("completed", first["status"]);
        let service = LocalService::resolve(home.path(), None).unwrap();
        let client = kuku_client::Client::local(&service).await.unwrap();
        let mut edit = Command::new(env!("CARGO_BIN_EXE_kuku-cli"));
        edit.env("KUKU_HOME", home.path()).env_remove(VARIABLE);
        if let Some(value) = editor_model {
            edit.env(VARIABLE, value);
        }
        let patch = "default_model = { connection = \"work\", model = \"model-B\" }\n[model.work.model-B]\nmax_output_tokens = 512";
        let edited = edit
            .args(["config", "set", "--patch", patch])
            .output()
            .unwrap();
        assert_eq!("updated", result(&edited)["status"]);
        tokio::time::timeout(Duration::from_secs(7), async {
            while client.models().await.unwrap().default_model.model != "model-B" {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("service should load the edited file");
        for args in [
            vec![
                "--json",
                "--session",
                first["session_id"].as_str().unwrap(),
                "continue fixture",
            ],
            vec!["--json", "new fixture"],
        ] {
            let output = invoke(workspace.path(), home.path(), &config, &args);
            assert_eq!("completed", result(&output)["status"]);
        }
        stop_managed(home.path()).await;
    }
    old_requests.assert_hits(4);
    new_requests.assert_hits(2);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigint_during_snapshot_respects_the_submission_boundary() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::extract::Request;
    use axum::middleware::{Next, from_fn};
    use tokio::sync::{Mutex, Notify};

    let provider = MockServer::start_async().await;
    super::mock_round_delayed(
        &provider,
        0,
        json!([{"type":"text","text":"Too late."}]),
        "end_turn",
        Duration::from_secs(10),
    );
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config_path = config_file(home.path(), provider.port(), provider.port());
    let config = kuku::config::load_config(&config_path)
        .unwrap()
        .resolve()
        .unwrap();
    let token = "signal-fixture-token";
    let state = Arc::new(kuku_server::AppState {
        run_manager: Mutex::new(kuku_server::run_manager::RunManager::new(1)),
        config: Arc::new(Arc::new(config).into()),
        password: Some(token.into()),
        kuku_home: home.path().to_owned(),
        authorizer: Arc::new(kuku::permission::TrustedLocal),
    });
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let delay_once = Arc::new(AtomicBool::new(true));
    let app = kuku_server::build_app(state).layer(from_fn({
        let arrived = Arc::clone(&arrived);
        let release = Arc::clone(&release);
        let delay_once = Arc::clone(&delay_once);
        move |request: Request, next: Next| {
            let arrived = Arc::clone(&arrived);
            let release = Arc::clone(&release);
            let delay_once = Arc::clone(&delay_once);
            async move {
                if request.uri().path().ends_with("/snapshot")
                    && delay_once.swap(false, Ordering::AcqRel)
                {
                    arrived.notify_one();
                    release.notified().await;
                }
                next.run(request).await
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let launch = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
            .env("KUKU_HOME", home.path())
            .arg("--workspace")
            .arg(workspace.path().canonicalize().unwrap())
            .args(["--remote", &endpoint, "--token", token, "run"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut child = launch(&[
        "--stream-json",
        "--command-id",
        "signal-input",
        "fixture prompt",
    ]);
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut started = String::new();
    stdout.read_line(&mut started).unwrap();
    let first: Value = serde_json::from_str(&started).unwrap();
    assert_eq!("started", first["event"]);
    tokio::time::timeout(Duration::from_secs(5), arrived.notified())
        .await
        .expect("initial snapshot must be in flight");
    let observer = kuku_client::Client::remote(&endpoint, token).unwrap();
    let target = kuku_client::Target {
        workspace: workspace.path().canonicalize().unwrap(),
        session_id: first["session_id"].as_str().unwrap().into(),
        agent: "main".into(),
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let receipt = observer
                .receipt(&target, "signal-input")
                .await
                .unwrap()
                .unwrap();
            if receipt.status == Some(kuku_client::InputStatus::Accepted) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("input must be admitted before SIGINT");
    assert!(
        Command::new("kill")
            .args(["-s", "INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    release.notify_one();
    let mut output = child.wait_with_output().unwrap();
    output.stdout = started.into_bytes();
    stdout.read_to_end(&mut output.stdout).unwrap();
    assert_eq!(
        Some(130),
        output.status.code(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = json_lines(&output);
    let final_record = records.last().unwrap();
    assert_eq!("cancelled", final_record["status"]);
    assert_eq!(first["session_id"], final_record["session_id"]);
    assert_eq!(first["receipt"], final_record["receipt"]);

    delay_once.store(true, Ordering::Release);
    let mut preflight = launch(&[
        "--stream-json",
        "--session",
        &target.session_id,
        "--command-id",
        "cancelled-before-submission",
        "do not submit this prompt",
    ]);
    tokio::time::timeout(Duration::from_secs(5), arrived.notified())
        .await
        .expect("the preflight snapshot must be in flight");
    assert!(
        Command::new("kill")
            .args(["-s", "INT", &preflight.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let finished = tokio::time::timeout(Duration::from_secs(5), async {
        while preflight.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    release.notify_one();
    if finished.is_err() {
        let _ = preflight.kill();
    }
    let output = preflight.wait_with_output().unwrap();
    finished.expect("preflight cancellation must not wait for the service response");
    let record = super::result_with_code(&output, 130);
    assert_eq!("cancelled", record["status"]);
    assert_eq!(target.session_id, record["session_id"]);
    assert!(record["receipt"].is_null());
    assert!(
        observer
            .receipt(&target, "cancelled-before-submission")
            .await
            .unwrap()
            .is_none()
    );
    server.abort();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigint_withdraws_this_queued_input_and_leaves_the_earlier_execution_running() {
    let provider = MockServer::start_async().await;
    let earlier_request = provider.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("begin earlier execution");
        then.status(200).delay(Duration::from_secs(10)).body(
            kuku::test_support::anthropic_sse_response(json!({
                "id":"earlier","type":"message","role":"assistant",
                "content":[{"type":"text","text":"Earlier response."}],
                "stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":4}
            })),
        );
    });
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = config_file(home.path(), provider.port(), provider.port());
    let mut earlier = run_cli(workspace.path(), home.path(), &config)
        .args(["--stream-json", "begin earlier execution"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut earlier_stdout = BufReader::new(earlier.stdout.take().unwrap());
    let mut started = String::new();
    earlier_stdout.read_line(&mut started).unwrap();
    let first: Value = serde_json::from_str(&started).unwrap();
    let session_id = first["session_id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while earlier_request.hits_async().await == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("earlier model request must be in flight");

    let service = LocalService::resolve(home.path(), None).unwrap();
    let client = kuku_client::Client::local(&service).await.unwrap();
    let target = kuku_client::Target {
        workspace: workspace.path().canonicalize().unwrap(),
        session_id: session_id.into(),
        agent: "main".into(),
    };
    let earlier_execution = client
        .snapshot(&target)
        .await
        .unwrap()
        .execution_id()
        .unwrap();

    let mut second = run_cli(workspace.path(), home.path(), &config)
        .args([
            "--stream-json",
            "--session",
            session_id,
            "--command-id",
            "withdraw-me",
            "queued steering",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut second_stdout = BufReader::new(second.stdout.take().unwrap());
    started.clear();
    second_stdout.read_line(&mut started).unwrap();
    let second_started: Value = serde_json::from_str(&started).unwrap();
    assert_eq!("started", second_started["event"]);
    let queued = client
        .receipt(&target, "withdraw-me")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(kuku_client::InputStatus::Queued), queued.status);
    assert!(
        Command::new("kill")
            .args(["-s", "INT", &second.id().to_string()])
            .status()
            .unwrap()
            .success()
    );

    let mut output = second.wait_with_output().unwrap();
    output.stdout = started.into_bytes();
    second_stdout.read_to_end(&mut output.stdout).unwrap();
    let state = client.snapshot(&target).await.unwrap();
    let submitted = client
        .receipt(&target, "withdraw-me")
        .await
        .unwrap()
        .unwrap();
    if let Some(execution_id) = state.execution_id() {
        client
            .submit(&kuku_client::Command {
                target,
                command_id: "fixture-cleanup".into(),
                action: kuku_client::Action::Cancel { execution_id },
            })
            .await
            .unwrap();
    }
    earlier.wait_with_output().unwrap();
    stop_managed(home.path()).await;

    assert_eq!(
        Some(130),
        output.status.code(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = json_lines(&output);
    let terminal = records.last().unwrap();
    assert_eq!("cancelled", terminal["status"]);
    assert_eq!("withdrawn", terminal["input_status"]);
    assert_eq!(second_started["receipt"], terminal["receipt"]);
    assert!(terminal["response"].is_null());
    assert_eq!(Some(kuku_client::InputStatus::Withdrawn), submitted.status);
    assert!(state.pending.is_empty());
    assert_eq!(AgentStatus::Running, state.agent.status);
    assert_eq!(Some(earlier_execution), state.execution_id());
}
