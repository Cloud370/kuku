mod common;

#[path = "../examples/session_orchestrate.rs"]
mod example;

use std::path::{Path, PathBuf};

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};

use kuku::{AgentStatus, Session, SessionOptions};

const INITIAL_MARKER: &str = "WRITER_PRIVATE_INITIAL_BRIEF";
const REVIEW_MARKER: &str = "REVIEWER_PRIVATE_INSTRUCTIONS";
const CONTINUE_MARKER: &str = "HOST_TIMER_CONTINUATION";
const DRAFT: &str = "DRAFT_SELECTED_MATERIAL";
const REVIEW: &str = "REVIEW_SELECTED_MATERIAL";

struct Workspace {
    root: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

impl Workspace {
    fn new(name: &str, server: &MockServer) -> Self {
        let (root, temporary) = match std::env::var_os("KUKU_ORCHESTRATE_EVIDENCE_DIR") {
            Some(path) => (PathBuf::from(path).join(name), None),
            None => {
                let temporary = tempfile::tempdir().unwrap();
                (temporary.path().to_path_buf(), Some(temporary))
            }
        };
        std::fs::create_dir_all(root.join("workspace")).unwrap();
        std::fs::write(root.join("workspace/answer.txt"), "before\n").unwrap();
        std::fs::write(root.join("draft.txt"), INITIAL_MARKER).unwrap();
        std::fs::write(root.join("review.txt"), REVIEW_MARKER).unwrap();
        std::fs::write(root.join("continue.txt"), CONTINUE_MARKER).unwrap();
        std::fs::write(root.join("config.toml"), config(server)).unwrap();
        Self {
            root,
            _temporary: temporary,
        }
    }

    fn arguments(&self) -> std::collections::BTreeMap<String, String> {
        let mut args = Vec::new();
        for (flag, path) in [
            ("workspace", "workspace"),
            ("data-dir", "session-data"),
            ("config", "config.toml"),
            ("prompt-file", "draft.txt"),
            ("review-prompt-file", "review.txt"),
            ("continue-prompt-file", "continue.txt"),
            ("output", "result.json"),
        ] {
            args.extend([
                format!("--{flag}"),
                self.root.join(path).to_string_lossy().into_owned(),
            ]);
        }
        args.extend(
            [
                "--writer-connection",
                "writer-route",
                "--writer-tier",
                "balanced",
                "--writer-model",
                "writer-test",
                "--writer-reasoning-effort",
                "low",
                "--writer-service-tier",
                "default",
                "--reviewer-connection",
                "reviewer-route",
                "--reviewer-tier",
                "light",
                "--reviewer-model",
                "reviewer-test",
                "--reviewer-reasoning-effort",
                "none",
                "--reviewer-service-tier",
                "priority",
                "--timer-ms",
                "60",
                "--timeout",
                "10",
                "--command-id",
                "controlled-discussion",
            ]
            .map(String::from),
        );
        example::arguments(args.into_iter()).unwrap()
    }

    fn report(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.root.join("result.json")).unwrap()).unwrap()
    }
}

fn config(server: &MockServer) -> String {
    format!(
        r#"default_model = "balanced"
[model.balanced]
provider = "writer-route"
model = "writer-tier-default"
think = "off"
context_window = 131072
max_output_tokens = 32768
[model.light]
provider = "reviewer-route"
model = "reviewer-tier-default"
think = "off"
context_window = 131072
max_output_tokens = 32768
[model.strong]
provider = "writer-route"
model = "writer-tier-default"
think = "off"
context_window = 131072
max_output_tokens = 32768
[provider.writer-route]
format = "openai-chat"
base_url = "{}"
api_key = "mock-only"
profile = "compatible"
reasoning_efforts = ["none", "low"]
service_tiers = ["default", "priority"]
[provider.reviewer-route]
format = "openai-chat"
base_url = "{}"
api_key = "mock-only"
profile = "compatible"
reasoning_efforts = ["none", "low"]
service_tiers = ["default", "priority"]
[discovery]
auto_discover = false
[handoff]
enabled = false
[plugin]
enabled = false
"#,
        server.url("/writer"),
        server.url("/reviewer")
    )
}

fn body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap()
}

fn tool_count(request: &HttpMockRequest) -> usize {
    body(request)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .count()
}

fn continuing(request: &HttpMockRequest) -> bool {
    body(request).to_string().contains(CONTINUE_MARKER)
}

fn tool(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}})
}

async fn fixture<'a>(
    server: &'a MockServer,
    path: &str,
    matcher: fn(&HttpMockRequest) -> bool,
    message: Value,
    finish_reason: &str,
) -> Mock<'a> {
    let response = common::openai_sse_response(
        json!({"id":"orchestrate-test","model":"orchestrate-test","choices":[{"message":message,"finish_reason":finish_reason}],"usage":{"prompt_tokens":100,"completion_tokens":20}}),
    );
    server
        .mock_async(move |when, then| {
            when.method(POST).path(path).matches(matcher);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response);
        })
        .await
}

fn command() -> &'static str {
    if cfg!(windows) {
        "type answer.txt"
    } else {
        "cat answer.txt"
    }
}

async fn writer_fixtures(server: &MockServer) -> Vec<Mock<'_>> {
    let read = fixture(
        server,
        "/writer/chat/completions",
        |request| tool_count(request) == 0 && body(request)["model"] == "writer-test",
        json!({"tool_calls":[tool("draft-read", "read_file", json!({"path":"answer.txt"}))]}),
        "tool_calls",
    )
    .await;
    let edit = fixture(server, "/writer/chat/completions", |request| tool_count(request) == 1, json!({"tool_calls":[tool("draft-edit", "edit_file", json!({"path":"answer.txt","old_text":"before","new_text":"after","brief":"apply draft"}))]}), "tool_calls").await;
    let check = fixture(
        server,
        "/writer/chat/completions",
        |request| tool_count(request) == 2,
        json!({"tool_calls":[tool("draft-check", "run_command", common::shell_args(command()))]}),
        "tool_calls",
    )
    .await;
    let draft = fixture(
        server,
        "/writer/chat/completions",
        |request| tool_count(request) == 3 && !continuing(request),
        json!({"content":DRAFT}),
        "stop",
    )
    .await;
    let reread = fixture(
        server,
        "/writer/chat/completions",
        |request| {
            let text = body(request).to_string();
            tool_count(request) == 3
                && continuing(request)
                && text.contains(INITIAL_MARKER)
                && text.contains(REVIEW)
                && !text.contains(REVIEW_MARKER)
        },
        json!({"tool_calls":[tool("timer-read", "read_file", json!({"path":"answer.txt"}))]}),
        "tool_calls",
    )
    .await;
    let revise = fixture(server, "/writer/chat/completions", |request| tool_count(request) == 4, json!({"tool_calls":[tool("timer-edit", "edit_file", json!({"path":"answer.txt","old_text":"after","new_text":"reviewed","brief":"apply review"}))]}), "tool_calls").await;
    let recheck = fixture(
        server,
        "/writer/chat/completions",
        |request| tool_count(request) == 5,
        json!({"tool_calls":[tool("timer-check", "run_command", common::shell_args(command()))]}),
        "tool_calls",
    )
    .await;
    let final_result = fixture(
        server,
        "/writer/chat/completions",
        |request| tool_count(request) == 6,
        json!({"content":"FINAL_SELECTED_MATERIAL"}),
        "stop",
    )
    .await;
    vec![
        read,
        edit,
        check,
        draft,
        reread,
        revise,
        recheck,
        final_result,
    ]
}

async fn reviewer_fixtures(server: &MockServer) -> Vec<Mock<'_>> {
    let initial = fixture(
        server,
        "/reviewer/chat/completions",
        |request| {
            let request_body = body(request).to_string();
            tool_count(request) == 0
                && request_body.contains(DRAFT)
                && request_body.contains(REVIEW_MARKER)
                && !request_body.contains(INITIAL_MARKER)
                && !request_body.contains("draft-read")
                && body(request)["model"] == "reviewer-test"
        },
        json!({"tool_calls":[tool("review-read", "read_file", json!({"path":"answer.txt"}))]}),
        "tool_calls",
    )
    .await;
    let result = fixture(
        server,
        "/reviewer/chat/completions",
        |request| tool_count(request) == 1,
        json!({"content":REVIEW}),
        "stop",
    )
    .await;
    vec![initial, result]
}

#[tokio::test]
async fn example_controls_independent_agents_and_timer_continuation_through_public_sdk() {
    let server = MockServer::start_async().await;
    let workspace = Workspace::new("public-path", &server);
    let writer = writer_fixtures(&server).await;
    let reviewer = reviewer_fixtures(&server).await;
    assert!(example::execute(workspace.arguments()).await.unwrap());
    let report = workspace.report();
    assert_eq!(true, report["runtime_completed"]);
    assert_eq!(true, report["cleanup"]["closed"]);
    assert_eq!(true, report["cleanup"]["persisted_snapshots_match"]);
    assert_eq!(
        true,
        report["orchestration"]["reopen"]["preserved_snapshots"]
    );
    assert_eq!(
        "reviewed\n",
        std::fs::read_to_string(workspace.root.join("workspace/answer.txt")).unwrap()
    );
    assert!(!workspace.root.join("workspace/.git").exists());
    assert_rounds(&report);
    assert_eq!(2, report["agents"].as_array().unwrap().len());
    assert_eq!(8, report["agent"]["usage"]["model_requests"]);
    assert_eq!(6, report["facts"]["tool_calls"]);
    assert_eq!(6, report["facts"]["tool_results"]);
    assert_eq!(2, report["facts"]["turns_completed"]);
    assert_eq!(
        2,
        report["other_agent_evidence"]["reviewer"]["facts"]["model_responses"]
    );
    assert_eq!(32768, report["limits"]["max_output_tokens"]);
    assert_eq!(131072, report["limits"]["max_context_tokens"]);
    assert_eq!(Value::Null, report["limits"]["max_request_bytes"]);
    assert_eq!(16, report["limits"]["max_requests_per_open"]);
    assert_eq!(2, report["limits"]["max_opens"]);
    let persisted =
        Session::read_agents(Path::new(report["events_path"].as_str().unwrap())).unwrap();
    assert_eq!(2, persisted.len());
    assert!(persisted
        .iter()
        .all(|agent| agent.status == AgentStatus::Idle && agent.pending_inputs == 0));
    assert!(
        report["orchestration"]["observation"]["committed"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        report["last_event_id"],
        report["orchestration"]["observation"]["cursor"]
    );
    assert_context_and_retained_results(&report);
    for mock in writer.iter().chain(reviewer.iter()) {
        mock.assert_hits_async(1).await;
    }
    println!("orchestration evidence: {}", workspace.root.display());
}

fn assert_rounds(report: &Value) {
    let rounds = report["orchestration"]["rounds"].as_array().unwrap();
    assert_eq!(3, rounds.len());
    assert_eq!("draft", rounds[0]["phase"]);
    assert_eq!("review", rounds[1]["phase"]);
    assert_eq!("timer_continuation", rounds[2]["phase"]);
    for round in rounds {
        assert_eq!(true, round["repeated_receipt_matches"]);
        assert_eq!(true, round["receipt_verified_after_settlement"]);
        assert_eq!("idle", round["snapshot"]["status"]);
        assert_eq!(0, round["snapshot"]["pending_inputs"]);
        assert_eq!("controlled-discussion", round["input"]["correlation"]);
    }
    assert_eq!("enqueue", rounds[0]["input"]["activation"]);
    assert_eq!("enqueue", rounds[1]["input"]["activation"]);
    assert_eq!("agent", rounds[1]["input"]["source"]["kind"]);
    assert_eq!("writer", rounds[1]["input"]["source"]["identity"]);
    assert_eq!("wake", rounds[2]["input"]["activation"]);
    assert_eq!("external", rounds[2]["input"]["source"]["kind"]);
    assert_eq!("timer", rounds[2]["input"]["source"]["identity"]);
    let timer = &report["orchestration"]["timer"];
    assert_eq!(timer["requests_before"], timer["requests_after_wait"]);
    assert!(timer["waited_ms"].as_u64().unwrap() >= 60);
}

fn assert_context_and_retained_results(report: &Value) {
    let settings = report["provider_settings"].as_array().unwrap();
    assert_eq!(8, settings.len());
    assert!(settings.iter().all(
        |setting| setting["connection"] == "writer-route" && setting["model"] == "writer-test"
    ));
    assert!(settings
        .iter()
        .all(|setting| setting["settings"]["reasoning_effort"] == "low"
            && setting["settings"]["service_tier"] == "default"));
    let reviewer = report["other_agent_evidence"]["reviewer"]["provider_settings"]
        .as_array()
        .unwrap();
    assert_eq!(2, reviewer.len());
    assert!(reviewer
        .iter()
        .all(|setting| setting["connection"] == "reviewer-route"
            && setting["model"] == "reviewer-test"));
    assert!(reviewer
        .iter()
        .all(|setting| setting["settings"]["reasoning_effort"] == "none"
            && setting["settings"]["service_tier"] == "priority"));
    let outputs = report["command_outputs"].as_array().unwrap();
    assert_eq!(2, outputs.len());
    let mut text = Vec::new();
    for output in outputs {
        assert_eq!(true, output["complete"]);
        assert_eq!(false, output["capture_truncated"]);
        assert_eq!(false, output["cleanup_incomplete"]);
        text.push(
            output["segments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|segment| segment["text"].as_str().unwrap())
                .collect::<String>(),
        );
        let path = Path::new(report["events_path"].as_str().unwrap())
            .parent()
            .unwrap()
            .join("outputs")
            .join(format!("{}.output", output["output_id"].as_str().unwrap()));
        assert!(path.is_file());
    }
    let text = text
        .into_iter()
        .map(|text| text.replace("\r\n", "\n"))
        .collect::<Vec<_>>();
    assert_eq!(vec!["after\n".to_string(), "reviewed\n".to_string()], text);
}

#[tokio::test]
async fn example_reports_waiting_as_incomplete_and_releases_the_session_writer() {
    let server = MockServer::start_async().await;
    let workspace = Workspace::new("waiting-is-not-success", &server);
    let question = fixture(&server, "/writer/chat/completions", |_| true, json!({"tool_calls":[tool("question", "ask_user", json!({"question":"Which result is approved?"}))]}), "tool_calls").await;
    assert!(!example::execute(workspace.arguments()).await.unwrap());
    let report = workspace.report();
    assert_eq!(false, report["runtime_completed"]);
    assert_eq!(true, report["cleanup"]["closed"]);
    assert_eq!("waiting", report["agent"]["status"]);
    assert!(report["error"].as_str().unwrap().contains("Waiting"));
    assert_eq!(
        0,
        report["orchestration"]["rounds"].as_array().unwrap().len()
    );
    let config = kuku::config::load_config(&workspace.root.join("config.toml"))
        .unwrap()
        .resolve()
        .unwrap();
    let mut options = SessionOptions::new(
        workspace.root.join("workspace"),
        workspace.root.join("session-data"),
        config,
    );
    options.session_id = Some(report["session_id"].as_str().unwrap().into());
    let reopened = Session::open(options).await.unwrap();
    assert_eq!(
        AgentStatus::Waiting,
        reopened.agent("writer").unwrap().status
    );
    assert!(reopened.agent("writer").unwrap().question.is_some());
    reopened.close().await.unwrap();
    question.assert_hits_async(1).await;
}

#[tokio::test]
async fn example_enforces_the_caller_request_cap_before_a_second_dispatch() {
    let server = MockServer::start_async().await;
    let workspace = Workspace::new("request-cap", &server);
    let writer = writer_fixtures(&server).await;
    let mut args = workspace.arguments();
    args.insert("max-requests".into(), "1".into());
    assert!(!example::execute(args).await.unwrap());
    let report = workspace.report();
    assert_eq!(false, report["runtime_completed"]);
    assert_eq!(1, report["limits"]["max_requests_per_open"]);
    assert!(report["error"].is_string());
    assert_eq!(
        "before\n",
        std::fs::read_to_string(workspace.root.join("workspace/answer.txt")).unwrap()
    );
    writer[0].assert_hits_async(1).await;
    for mock in &writer[1..] {
        mock.assert_hits_async(0).await;
    }
}
