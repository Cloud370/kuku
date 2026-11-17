mod common;

use std::sync::Arc;
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};

use kuku::event::EventPayload;
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    Activation, AgentConfig, AgentSnapshot, AgentStatus, Input, OutputRead, Session, SessionOptions,
};

fn options(
    server: &MockServer,
    workspace: &std::path::Path,
    data: &std::path::Path,
) -> SessionOptions {
    let mut config = common::test_config();
    config.providers.get_mut("openai").unwrap().base_url = server.url("");
    config.tiers.get_mut("balanced").unwrap().provider = "openai".into();
    config.tiers.get_mut("balanced").unwrap().model = "runtime-test".into();
    config.tiers.get_mut("balanced").unwrap().think = kuku::config::ThinkLevel::Off;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    let mut options = SessionOptions::new(workspace, data, config);
    options.limits.max_steps_per_turn = 8;
    options
}

fn tool_count(request: &HttpMockRequest) -> usize {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .count()
}

fn response(message: Value, finish_reason: &str) -> String {
    common::openai_sse_response(
        json!({"id":"runtime-test","model":"runtime-test","choices":[{"message":message,"finish_reason":finish_reason}],"usage":{"prompt_tokens":100,"completion_tokens":20}}),
    )
}

fn tool(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
}

async fn fixture<'a>(
    server: &'a MockServer,
    matcher: fn(&HttpMockRequest) -> bool,
    message: Value,
    reason: &str,
) -> Mock<'a> {
    let body = response(message, reason);
    server
        .mock_async(move |when, then| {
            when.method(POST).path("/chat/completions").matches(matcher);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(body);
        })
        .await
}

async fn settled(session: &Session, address: &str) -> AgentSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = session.agent(address).unwrap();
            if state.status != AgentStatus::Running {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent did not settle")
}

#[tokio::test]
async fn public_session_executes_real_tools_without_observers_and_reopens() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("answer.txt"), "before\n").unwrap();
    let first = fixture(&server, |request| tool_count(request) == 0, json!({"tool_calls":[tool("read", "read_file", json!({"path":"answer.txt"})), tool("find", "find_files", json!({"path":"."}))]}), "tool_calls").await;
    let second = fixture(&server, |request| tool_count(request) == 2, json!({"tool_calls":[tool("edit", "edit_file", json!({"path":"answer.txt","old_text":"before","new_text":"after","brief":"correct file"}))]}), "tool_calls").await;
    let command = if cfg!(windows) {
        "type answer.txt"
    } else {
        "cat answer.txt"
    };
    let third = fixture(&server, |request| tool_count(request) == 3, json!({"tool_calls":[tool("command", "run_command", json!({"command":command,"timeout":3,"brief":"check result"}))]}), "tool_calls").await;
    let last = fixture(
        &server,
        |request| tool_count(request) == 4,
        json!({"content":"done"}),
        "stop",
    )
    .await;
    let original = options(&server, workspace.path(), data.path());
    let session = Session::open(original.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "input",
            "main",
            "correct answer.txt and check it",
        ))
        .unwrap();
    let state = settled(&session, "main").await;
    assert_eq!(AgentStatus::Idle, state.status, "{:?}", state.last_error);
    assert_eq!(Some("done"), state.last_output.as_deref());
    assert_eq!(
        "after\n",
        std::fs::read_to_string(workspace.path().join("answer.txt")).unwrap()
    );
    assert!(!workspace.path().join(".git").exists());
    let events = session.events(0, 256).unwrap();
    let output_id = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolResult {
                structured: Some(value),
                ..
            } if value["kind"] == "command_result" => value["output_id"].as_str(),
            _ => None,
        })
        .expect("command did not commit a retained output reference");
    let output = session
        .read_output(output_id, &OutputRead::default())
        .unwrap();
    assert!(output.complete);
    assert!(!output.capture_truncated);
    assert!(output
        .segments
        .iter()
        .any(|segment| segment.text.contains("after")));
    let id = session.id().to_string();
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();
    assert_eq!(1, Session::read_agents(&events_path).unwrap().len());
    let mut reopen = original;
    reopen.session_id = Some(id);
    let reopened = Session::open(reopen).await.unwrap();
    reopened
        .send(Input::new("followup", "main", "verify the existing result"))
        .unwrap();
    assert_eq!(AgentStatus::Idle, settled(&reopened, "main").await.status);
    reopened.close().await.unwrap();
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
    third.assert_hits_async(1).await;
    last.assert_hits_async(2).await;
}

#[tokio::test]
async fn receipts_are_durable_idempotent_and_enqueue_requires_explicit_advance() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = fixture(&server, |_| true, json!({"content":"accepted"}), "stop").await;
    let original = options(&server, workspace.path(), data.path());
    let session = Session::open(original.clone()).await.unwrap();
    let create = session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    assert_eq!(
        create,
        session
            .create_agent("create", "main", AgentConfig::default())
            .unwrap()
    );
    assert!(session
        .create_agent("another", "main", AgentConfig::default())
        .is_err());
    assert!(session
        .create_agent("nested", "review/child", AgentConfig::default())
        .is_err());
    let mut input = Input::new("input", "main", "pending");
    input.activation = Activation::Enqueue;
    let receipt = session.send(input.clone()).unwrap();
    assert_eq!(receipt, session.send(input.clone()).unwrap());
    input.text = "conflicting retry".into();
    assert!(session.send(input).is_err());
    assert_eq!(AgentStatus::Idle, session.agent("main").unwrap().status);
    model.assert_hits_async(0).await;
    let mut reopen = original;
    reopen.session_id = Some(session.id().into());
    assert!(Session::open(reopen.clone()).await.is_err());
    session.close().await.unwrap();
    let reopened = Session::open(reopen).await.unwrap();
    assert_eq!(1, reopened.agent("main").unwrap().pending_inputs);
    model.assert_hits_async(0).await;
    reopened.advance("main").unwrap();
    assert_eq!(AgentStatus::Idle, settled(&reopened, "main").await.status);
    reopened.close().await.unwrap();
    model.assert_hits_async(1).await;
}

#[derive(Debug)]
struct Deferred;

impl ToolAuthorizer for Deferred {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        assert_eq!("main", action.agent);
        assert_eq!("run_command", action.tool.name);
        assert_eq!("echo authorized", action.args["command"]);
        Ok(AuthorizationDecision::Defer)
    }
}

#[tokio::test]
async fn exact_authorization_survives_close_and_rejects_stale_replies() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = fixture(&server, |request| tool_count(request) == 0, json!({"tool_calls":[tool("command", "run_command", json!({"command":"echo authorized","timeout":2,"brief":"check permission"}))]}), "tool_calls").await;
    let last = fixture(
        &server,
        |request| tool_count(request) == 1,
        json!({"content":"authorized"}),
        "stop",
    )
    .await;
    let mut original = options(&server, workspace.path(), data.path());
    original.authorizer = Arc::new(Deferred);
    let session = Session::open(original.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "run exact command"))
        .unwrap();
    let waiting = settled(&session, "main").await;
    assert_eq!(
        AgentStatus::Waiting,
        waiting.status,
        "{:?}",
        waiting.last_error
    );
    let request = waiting.permission.unwrap();
    assert!(session
        .reply_permission("wrong", "main", "unrelated", true)
        .is_err());
    let mut reopen = original;
    reopen.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let session = Session::open(reopen).await.unwrap();
    let receipt = session
        .reply_permission("reply", "main", &request.id, true)
        .unwrap();
    assert_eq!(
        receipt,
        session
            .reply_permission("reply", "main", &request.id, true)
            .unwrap()
    );
    let done = settled(&session, "main").await;
    assert_eq!(AgentStatus::Idle, done.status, "{:?}", done.last_error);
    assert!(session
        .reply_permission("stale", "main", &request.id, true)
        .is_err());
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    last.assert_hits_async(1).await;
}
