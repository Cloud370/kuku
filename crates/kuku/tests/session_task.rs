mod common;

#[path = "../examples/session_task.rs"]
mod example;

use std::fs;
use std::path::Path;

use httpmock::prelude::{MockServer, POST};
use serde_json::{json, Value};

use kuku::event::{EventPayload, EventStore};
use kuku::Session;

const CONTEXT_TOKENS: u32 = 16_384;
#[tokio::test]
async fn example_separates_request_bytes_from_context_tokens_and_reports_settlement() {
    let server = MockServer::start_async().await;
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    prepare(
        root,
        &server,
        &"A line of example task input.\n".repeat(1024),
    );
    let response = common::openai_sse_response(json!({
        "choices": [{"message": {"content": "done"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 7000, "completion_tokens": 1},
    }));
    let provider = server
        .mock_async(move |when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response);
        })
        .await;

    let failed = run_example(
        root,
        "failed.json",
        &["--max-request-bytes", &CONTEXT_TOKENS.to_string()],
        false,
    )
    .await;
    assert_eq!(CONTEXT_TOKENS, failed["limits"]["max_context_tokens"]);
    assert_eq!(CONTEXT_TOKENS, failed["limits"]["max_request_bytes"]);
    assert_eq!("interrupted", failed["agent"]["status"]);
    assert_eq!(failed["agent"]["last_error"], failed["error"]);
    let error = failed["error"].as_str().unwrap();
    assert!(error.contains("prepared model request exceeds the configured byte limit"));
    assert!(error.ends_with(&format!("bytes, limit {CONTEXT_TOKENS} bytes)")));
    assert_eq!(0, failed["agent"]["usage"]["model_requests"]);
    assert_eq!(json!([]), failed["provider_settings"]);
    assert_terminal(&failed, "turn.interrupted");
    provider.assert_hits_async(0).await;

    let completed = run_example(root, "completed.json", &[], true).await;
    let request_byte_limit = kuku::RuntimeLimits::default().max_request_bytes;
    assert_eq!(CONTEXT_TOKENS, completed["limits"]["max_context_tokens"]);
    assert_eq!(request_byte_limit, completed["limits"]["max_request_bytes"]);
    assert_eq!("idle", completed["agent"]["status"]);
    assert_eq!("done", completed["agent"]["last_output"]);
    assert_eq!(Value::Null, completed["error"]);
    assert_eq!(1, completed["agent"]["usage"]["model_requests"]);
    let settings = &completed["provider_settings"][0];
    assert_eq!(CONTEXT_TOKENS, settings["context_window_tokens"]);
    assert_eq!(request_byte_limit, settings["max_request_bytes"]);
    assert!(settings["request_bytes"].as_u64().unwrap() > u64::from(CONTEXT_TOKENS));
    assert_terminal(&completed, "turn.completed");
    provider.assert_hits_async(1).await;
}

#[tokio::test]
async fn example_inherits_retry_policy_and_bounds_attempts_only_when_requested() {
    let server = MockServer::start_async().await;
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    prepare(root, &server, "finish the task");
    let provider = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(503).body("provider temporarily unavailable");
        })
        .await;

    let default = run_example(root, "default.json", &[], false).await;
    assert_eq!(Value::Null, default["limits"]["max_requests"]);
    assert_eq!(1, default["limits"]["automatic_retries"]);
    assert_eq!(2, default["agent"]["usage"]["model_requests"]);
    provider.assert_hits_async(2).await;

    let capped = run_example(root, "capped.json", &["--max-requests", "1"], false).await;
    assert_eq!(1, capped["limits"]["max_requests"]);
    assert_eq!(1, capped["limits"]["automatic_retries"]);
    assert_eq!(1, capped["agent"]["usage"]["model_requests"]);
    provider.assert_hits_async(3).await;

    let no_retry = run_example(root, "no-retry.json", &["--no-provider-retry"], false).await;
    assert_eq!(0, no_retry["limits"]["automatic_retries"]);
    assert_eq!(1, no_retry["agent"]["usage"]["model_requests"]);
    provider.assert_hits_async(4).await;
}

#[tokio::test]
async fn temporary_rate_limit_retries_once_and_terminal_limits_return_control() {
    let server = MockServer::start_async().await;
    let root = tempfile::tempdir().unwrap();
    prepare(root.path(), &server, "finish the task");
    let budget = ["--max-requests", "1"];
    for (header, body, attempts, options) in [
        (
            "0",
            r#"{"error":{"code":"rate_limit_exceeded"}}"#,
            2,
            &[][..],
        ),
        (
            "0",
            r#"{"error":{"code":"insufficient_quota"}}"#,
            1,
            &[][..],
        ),
        (
            "120",
            r#"{"error":{"code":"rate_limit_exceeded"}}"#,
            1,
            &[][..],
        ),
        (
            "60",
            r#"{"error":{"code":"rate_limit_exceeded"}}"#,
            1,
            &budget[..],
        ),
    ] {
        let provider = server
            .mock_async(move |when, then| {
                when.method(POST).path("/chat/completions");
                then.status(429).header("retry-after", header).body(body);
            })
            .await;
        let output = format!("rate-{attempts}-{header}.json");
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run_example(root.path(), &output, options, false),
        )
        .await
        .expect("terminal failures must return control without a retry delay");
        provider.assert_hits_async(attempts).await;
        assert_eq!(
            attempts,
            report["agent"]["usage"]["model_requests"].as_u64().unwrap() as usize
        );
        if header == "120" {
            assert!(report["error"]
                .as_str()
                .unwrap()
                .contains("Retry-After: 120s"));
        }
        provider.delete_async().await;
    }
}

fn prepare(root: &Path, server: &MockServer, prompt: &str) {
    fs::create_dir(root.join("workspace")).unwrap();
    fs::write(root.join("config.toml"), config(server)).unwrap();
    fs::write(root.join("prompt.txt"), prompt).unwrap();
}

async fn run_example(root: &Path, output: &str, extra_args: &[&str], passed: bool) -> Value {
    let mut args = Vec::new();
    for (flag, path) in [
        ("--workspace", "workspace"),
        ("--data-dir", "session-data"),
        ("--config", "config.toml"),
        ("--prompt-file", "prompt.txt"),
        ("--output", output),
    ] {
        args.extend([flag.into(), root.join(path).to_str().unwrap().into()]);
    }
    args.extend([
        "--max-context-tokens".into(),
        CONTEXT_TOKENS.to_string(),
        "--max-output-tokens".into(),
        "1024".into(),
        "--timeout".into(),
        "10".into(),
        "--no-agents".into(),
        "--no-skills".into(),
    ]);
    args.extend(extra_args.iter().map(|value| (*value).to_owned()));
    let args = example::arguments(args.into_iter()).unwrap();
    assert_eq!(passed, example::execute(args).await.unwrap());
    let report: Value = serde_json::from_slice(&fs::read(root.join(output)).unwrap()).unwrap();
    assert_eq!(passed, report["runtime_completed"]);
    assert_eq!(true, report["cleanup"]["closed"]);
    report
}

fn assert_terminal(report: &Value, terminal_kind: &str) {
    let path = Path::new(report["events_path"].as_str().unwrap());
    let persisted = Session::read_agents(path).unwrap();
    assert_eq!(report["agents"], json!(persisted));
    let events = EventStore::replay(path).unwrap();
    let terminals = events
        .iter()
        .filter(|event| match &event.payload {
            EventPayload::TurnCompleted {
                conversation, turn, ..
            }
            | EventPayload::TurnInterrupted {
                conversation, turn, ..
            }
            | EventPayload::TurnCancelled {
                conversation, turn, ..
            } => conversation == "main" && report["agent"]["turn"] == *turn,
            _ => false,
        })
        .collect::<Vec<_>>();
    assert_eq!(1, terminals.len());
    assert_eq!(terminal_kind, terminals[0].payload.kind_name());
    let settlements = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["operation"] == "turn_settled" =>
            {
                Some((event.id, &value["record"]))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(1, settlements.len());
    let (id, settlement) = settlements[0];
    assert!(terminals[0].id < id);
    assert_eq!(report["agent"]["turn"], settlement["turn"]);
    assert_eq!(report["agent"]["status"], settlement["status"]);
    assert_eq!(report["error"], settlement["error"]);
}

fn config(server: &MockServer) -> String {
    let mut config = format!(
        r#"[provider.test-route]
format = "openai-chat"
base_url = "{}"
api_key = "mock-only"
profile = "compatible"
[discovery]
auto_discover = false
[handoff]
enabled = false
[plugin]
enabled = false
"#,
        server.url("")
    );
    for tier in ["light", "balanced", "strong"] {
        config.push_str(&format!(
            r#"[model.{tier}]
provider = "test-route"
model = "session-task-fixture"
think = "off"
context_window = 65536
max_output_tokens = 1024
"#
        ));
    }
    config
}
