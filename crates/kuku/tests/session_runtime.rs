#[path = "session_runtime/authorization.rs"]
mod authorization;
mod common;
#[path = "session_runtime/permission_resume.rs"]
mod permission_resume;
#[path = "session_runtime/unpersisted_resume.rs"]
mod unpersisted_resume;
#[path = "session_runtime/wait_support.rs"]
mod wait_support;

#[cfg(unix)]
use std::sync::Arc;
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};

use kuku::event::EventPayload;
#[cfg(unix)]
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{Activation, AgentConfig, AgentStatus, Input, OutputRead, Session, SessionOptions};
#[cfg(unix)]
use wait_support::wait_for_agent_during;
use wait_support::{settled, settled_during, settled_with_output, settled_with_timeout};

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
    SessionOptions::new(workspace, data, config)
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

fn user_message_count(body: &Value, text: &str) -> usize {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user" && message["content"] == text)
        .count()
}

fn request_has_original_input_once(request: &HttpMockRequest) -> bool {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
    user_message_count(&body, "retain this accepted work") == 1
}

fn request_has_original_input_twice(request: &HttpMockRequest) -> bool {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
    user_message_count(&body, "retain this accepted work") == 2
}

fn request_has_original_and_queued_input_once(request: &HttpMockRequest) -> bool {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
    user_message_count(&body, "retain this accepted work") == 1
        && user_message_count(&body, "queued during interruption") == 1
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
    let third = fixture(&server, |request| tool_count(request) == 3, json!({"tool_calls":[tool("command", "run_command", json!({"shell":if cfg!(windows) {"cmd"} else {"sh"},"cwd":".","command":command,"timeout":3,"brief":"check result"}))]}), "tool_calls").await;
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
    let state = settled_during(&session, "main", "initial execution").await;
    assert_eq!(AgentStatus::Idle, state.status, "{:?}", state.last_error);
    assert_eq!(Some("done"), state.last_output.as_deref());
    assert_eq!(4, state.usage.model_requests);
    assert_eq!(Some(400), state.usage.input_tokens);
    assert_eq!(Some(80), state.usage.output_tokens);
    assert_eq!(
        Some(100),
        state.usage.latest_request.as_ref().unwrap().input_tokens
    );
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
    assert_eq!(state.usage, reopened.agent("main").unwrap().usage);
    reopened
        .send(Input::new("followup", "main", "verify the existing result"))
        .unwrap();
    assert_eq!(
        AgentStatus::Idle,
        settled_during(&reopened, "main", "reopened execution")
            .await
            .status
    );
    reopened.close().await.unwrap();
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
    third.assert_hits_async(1).await;
    last.assert_hits_async(2).await;
}

#[tokio::test]
async fn provider_measurements_are_request_bound_and_replay_per_agent() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut original = options(&server, workspace.path(), data.path());
    original
        .config
        .tiers
        .get_mut("balanced")
        .unwrap()
        .context_window = 100_000;
    let mut fixtures = Vec::new();
    for (model, usage) in [
        (
            "complete",
            json!({
                "prompt_tokens": 10_000,
                "completion_tokens": 50,
                "prompt_tokens_details": {"cached_tokens": 0},
                "completion_tokens_details": {"reasoning_tokens": 35},
            }),
        ),
        ("output-only", json!({"completion_tokens": 30})),
        ("input-only", json!({"prompt_tokens": 20_000})),
    ] {
        let expected_body = json!({"model": model}).to_string();
        let body = common::openai_sse_response(json!({
            "id": "usage-fixture", "model": model, "usage": usage,
            "choices": [{"message": {"content": "measured"}, "finish_reason": "stop"}],
        }));
        fixtures.push(
            server
                .mock_async(move |when, then| {
                    when.method(POST)
                        .path("/chat/completions")
                        .json_body_partial(expected_body);
                    then.status(200)
                        .header("content-type", "text/event-stream")
                        .body(body);
                })
                .await,
        );
    }
    let session = Session::open(original.clone()).await.unwrap();
    for (agent, model) in [("main", "complete"), ("reviewer", "output-only")] {
        session
            .create_agent(
                &format!("create:{agent}"),
                agent,
                AgentConfig {
                    model: Some(model.into()),
                    ..AgentConfig::default()
                },
            )
            .unwrap();
        session
            .send(Input::new(format!("first:{agent}"), agent, "measure usage"))
            .unwrap();
    }
    let main = settled(&session, "main").await.usage;
    let reviewer = settled(&session, "reviewer").await.usage;
    assert_eq!(Some(10.0), main.context_percent());
    assert_eq!(Some(0.0), main.cache_hit_percent());
    assert_eq!(Some(50), main.output_tokens);
    assert!(main.input_tokens_complete() && main.output_tokens_complete());
    let rate = main.average_output_tokens_per_second();
    let sample = main.latest_request.as_ref().unwrap();
    assert_eq!(
        Some(50.0 * 1_000_000.0 / sample.elapsed_us.unwrap() as f64),
        rate
    );
    assert_eq!(rate, main.output_tokens_per_second());
    assert_eq!(None, reviewer.context_percent());
    assert_eq!(None, reviewer.input_tokens);
    assert_eq!(Some(30), reviewer.output_tokens);
    assert!(!reviewer.input_tokens_complete() && reviewer.output_tokens_complete());

    session
        .configure_agent(
            "new-route",
            "main",
            AgentConfig {
                model: Some("input-only".into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    assert_eq!(None, session.agent("main").unwrap().usage.latest_request);
    session
        .send(Input::new("second:main", "main", "measure the new route"))
        .unwrap();
    let main = settled(&session, "main").await.usage;
    assert_eq!(2, main.model_requests);
    assert_eq!(Some(30_000), main.input_tokens);
    assert_eq!(Some(50), main.output_tokens);
    assert!(main.input_tokens_complete() && !main.output_tokens_complete());
    assert_eq!(Some(20.0), main.context_percent());
    assert_eq!(None, main.output_tokens_per_second());
    assert_eq!(rate, main.average_output_tokens_per_second());
    assert_eq!(Some(0.0), main.cache_hit_percent());
    assert_eq!(10_000, main.cache_measured_input_tokens);
    assert_eq!(0, main.cache_measured_read_tokens);
    assert_eq!(reviewer, session.agent("reviewer").unwrap().usage);

    original.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let reopened = Session::open(original).await.unwrap();
    assert_eq!(main, reopened.agent("main").unwrap().usage);
    assert_eq!(reviewer, reopened.agent("reviewer").unwrap().usage);
    reopened.close().await.unwrap();
    for fixture in fixtures {
        fixture.assert_hits_async(1).await;
    }
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
    assert_eq!(Some(receipt.clone()), session.receipt("input").unwrap());
    assert_eq!(
        Some(receipt.clone()),
        session.input_receipt(&input).unwrap()
    );
    assert_eq!(receipt, session.send(input.clone()).unwrap());
    input.text = "conflicting retry".into();
    assert!(session.input_receipt(&input).is_err());
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

#[tokio::test]
async fn settled_provider_failure_allows_close_and_explicit_advance_after_reopen() {
    retry_interrupted_input(true, 400, 1, true, 0, false).await;
}

#[tokio::test]
async fn repeated_provider_failure_and_reopen_keeps_one_original_user_message() {
    retry_interrupted_input(true, 400, 1, true, 1, false).await;
}

#[tokio::test]
async fn queued_input_joins_restored_accepted_input_without_duplication() {
    retry_interrupted_input(true, 400, 1, true, 0, true).await;
}

#[tokio::test]
async fn successful_explicit_retry_after_rate_limiting_can_close_the_same_session() {
    retry_interrupted_input(false, 429, 1, false, 0, false).await;
}

#[tokio::test]
async fn exhausted_provider_retries_can_continue_without_duplicating_input() {
    retry_interrupted_input(false, 429, 6, true, 0, false).await;
}

async fn retry_interrupted_input(
    reopen: bool,
    status: u16,
    failed_attempts: usize,
    automatic_retries: bool,
    continued_failures: usize,
    queue_new_input: bool,
) {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let failure = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/chat/completions")
                .matches(request_has_original_input_once);
            then.status(status).body("provider fixture failure");
        })
        .await;
    let mut original = options(&server, workspace.path(), data.path());
    original.limits.retry_transient_provider_errors = automatic_retries;
    let session = Session::open(original.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "retain this accepted work"))
        .unwrap();
    let failed = if automatic_retries && status == 429 {
        settled_with_timeout(&session, "main", Duration::from_secs(90)).await
    } else {
        settled(&session, "main").await
    };
    assert_eq!(AgentStatus::Interrupted, failed.status);
    assert!(
        failed
            .last_error
            .as_ref()
            .is_some_and(|error| error.contains(&format!("HTTP {status}"))),
        "{failed:?}"
    );
    assert!(session.active_executions().is_empty());
    assert_eq!(failed_attempts as u64, failed.usage.model_requests);
    assert_eq!(failed_attempts as u64, failed.usage.unreported_requests);
    assert_eq!(None, failed.usage.input_tokens);
    assert_eq!(0, failed.pending_inputs);
    assert_eq!(
        1,
        session
            .events(0, 128)
            .unwrap()
            .iter()
            .filter(|event| matches!(&event.payload,
                EventPayload::MessageUser { text, .. } if text == "retain this accepted work"
            ))
            .count()
    );
    assert_eq!(
        1,
        session
            .events(0, 128)
            .unwrap()
            .iter()
            .filter(|event| matches!(&event.payload,
                EventPayload::TurnInterrupted { conversation, turn, .. }
                    if conversation == "main" && Some(*turn) == failed.turn
            ))
            .count(),
        "provider failure already records its own interruption"
    );
    let mut reopen_options = None;
    let mut session = if reopen {
        let mut options = original.clone();
        options.session_id = Some(session.id().into());
        session.close().await.unwrap();
        let reopened = Session::open(options.clone()).await.unwrap();
        assert_eq!(failed, reopened.agent("main").unwrap());
        reopen_options = Some(options);
        reopened
    } else {
        session
    };

    for _ in 0..continued_failures {
        session.advance("main").unwrap();
        let failed_again = settled(&session, "main").await;
        assert_eq!(AgentStatus::Interrupted, failed_again.status);
        assert_eq!(0, failed_again.pending_inputs);
        assert_eq!(
            1,
            session
                .events(0, 128)
                .unwrap()
                .iter()
                .filter(|event| matches!(&event.payload,
                    EventPayload::MessageUser { text, .. } if text == "retain this accepted work"
                ))
                .count()
        );
        if let Some(options) = &reopen_options {
            session.close().await.unwrap();
            let reopened = Session::open(options.clone()).await.unwrap();
            assert_eq!(failed_again, reopened.agent("main").unwrap());
            session = reopened;
        }
    }

    if queue_new_input {
        session
            .send(Input::new(
                "queued-during-interruption",
                "main",
                "queued during interruption",
            ))
            .unwrap();
        assert_eq!(1, session.agent("main").unwrap().pending_inputs);
    }
    failure
        .assert_hits_async(failed_attempts + continued_failures)
        .await;
    failure.delete_async().await;
    let success_matcher: fn(&HttpMockRequest) -> bool = if queue_new_input {
        request_has_original_and_queued_input_once
    } else {
        request_has_original_input_once
    };
    let success = fixture(
        &server,
        success_matcher,
        json!({"content":"retried"}),
        "stop",
    )
    .await;
    session.advance("main").unwrap();
    let done = settled(&session, "main").await;
    assert_eq!(
        1,
        session
            .events(0, 128)
            .unwrap()
            .iter()
            .filter(|event| matches!(&event.payload,
                EventPayload::MessageUser { text, .. } if text == "retain this accepted work"
            ))
            .count()
    );
    if queue_new_input {
        assert_eq!(
            1,
            session
                .events(0, 128)
                .unwrap()
                .iter()
                .filter(|event| matches!(&event.payload,
                    EventPayload::MessageUser { text, .. } if text == "queued during interruption"
                ))
                .count()
        );
    }
    assert_eq!(AgentStatus::Idle, done.status, "{:?}", done.last_error);
    assert_eq!(Some("retried"), done.last_output.as_deref());
    success.assert_hits_async(1).await;
    success.delete_async().await;

    let same_text = fixture(
        &server,
        request_has_original_input_twice,
        json!({"content":"same text persisted"}),
        "stop",
    )
    .await;
    session
        .send(Input::new(
            "same-text-new-command",
            "main",
            "retain this accepted work",
        ))
        .unwrap();
    let done = settled_with_output(&session, "main", "same text persisted").await;
    assert_eq!(AgentStatus::Idle, done.status, "{:?}", done.last_error);
    assert_eq!(
        2,
        session
            .events(0, 128)
            .unwrap()
            .iter()
            .filter(|event| matches!(&event.payload,
                EventPayload::MessageUser { text, .. } if text == "retain this accepted work"
            ))
            .count()
    );
    same_text.assert_hits_async(1).await;
    same_text.delete_async().await;

    let next = fixture(
        &server,
        |request| {
            String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                .contains("a distinct new submission")
        },
        json!({"content":"next input persisted"}),
        "stop",
    )
    .await;
    session
        .send(Input::new(
            "next-input",
            "main",
            "a distinct new submission",
        ))
        .unwrap();
    let done = settled_with_output(&session, "main", "next input persisted").await;
    assert_eq!(AgentStatus::Idle, done.status, "{:?}", done.last_error);
    assert_eq!(Some("next input persisted"), done.last_output.as_deref());
    let user_messages = session
        .events(0, 128)
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::MessageUser { text, .. } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        2,
        user_messages
            .iter()
            .filter(|text| *text == "retain this accepted work")
            .count()
    );
    assert_eq!(
        1,
        user_messages
            .iter()
            .filter(|text| *text == "a distinct new submission")
            .count()
    );
    if queue_new_input {
        assert_eq!(
            1,
            user_messages
                .iter()
                .filter(|text| *text == "queued during interruption")
                .count()
        );
    }
    session.close().await.unwrap();
    next.assert_hits_async(1).await;
}

#[cfg(unix)]
#[derive(Debug)]
struct PolicyFault(std::path::PathBuf);

#[cfg(unix)]
impl ToolAuthorizer for PolicyFault {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        if action.tool.name == "run_command" {
            std::fs::write(&self.0, "- invalid rule outside section\n")?;
        }
        Ok(AuthorizationDecision::Allow)
    }
}

#[cfg(unix)]
#[tokio::test]
async fn command_output_failure_after_a_side_effect_stops_model_dispatch() {
    for (operation, rename) in [
        ("sync", "mv capture/outputs capture/retired"),
        (
            "read",
            "mv capture/outputs/*.output capture/outputs/retired",
        ),
    ] {
        let server = MockServer::start_async().await;
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let session = Session::open(options(&server, workspace.path(), data.path()))
            .await
            .unwrap();
        std::os::unix::fs::symlink(
            session.events_path().parent().unwrap(),
            workspace.path().join("capture"),
        )
        .unwrap();
        let first = fixture(
            &server,
            |r| tool_count(r) == 0,
            json!({"tool_calls":[tool("command","run_command",json!({
                "shell":"sh","cwd":".","command":format!("printf ran > ran.txt && {rename} && printf captured"),
                "timeout": 5, "brief": "exercise command output recovery",
            }))]}),
            "tool_calls",
        )
        .await;
        let next = fixture(
            &server,
            |r| tool_count(r) > 0,
            json!({"content":"continued"}),
            "stop",
        )
        .await;
        session
            .create_agent("create", "main", AgentConfig::default())
            .unwrap();
        session
            .send(Input::new("input", "main", "run command"))
            .unwrap();
        let wait_error = wait_for_agent_during(&session, "main", "command output failure")
            .await
            .unwrap_err();
        assert!(wait_error
            .to_string()
            .contains("tool output could not be durably captured"));
        let snapshot = session.agent("main").unwrap();
        assert_eq!(
            "ran",
            std::fs::read_to_string(workspace.path().join("ran.txt")).unwrap()
        );
        assert_eq!(
            AgentStatus::Interrupted,
            snapshot.status,
            "{operation}: {snapshot:?}"
        );
        let error = session.close().await.unwrap_err().to_string();
        assert!(
            error.contains("tool output could not be durably captured"),
            "{error}"
        );
        first.assert_hits_async(1).await;
        next.assert_hits_async(0).await;
        assert!(session.events(0, 256).unwrap().iter().any(|event| matches!(&event.payload,
            EventPayload::ToolResult { tool_call_id, status, summary, structured: Some(value), .. }
                if tool_call_id == "command" && status == "error" && summary.starts_with(&format!("failed: output {operation}:"))
                    && value["output_persistence_failed"] == true && value["cleanup_incomplete"] == false
                    && value.get("output_id").is_none() && value.get("output_ref").is_none()
        )), "{operation} failure must leave a durable result without an output reference");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn execution_error_drains_dispatched_command_and_preserves_reported_usage() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let _model = fixture(&server, |_| true, json!({"tool_calls":[tool("command", "run_command", json!({"shell":"sh","cwd":".","command":"sleep 0.3; echo escaped > after-close.txt","timeout":3,"brief":"write after delay"})),tool("agents", "list_agents", json!({}))]}), "tool_calls").await;
    let mut opts = options(&server, workspace.path(), data.path());
    let policy = kuku::session::project_policy_path(data.path(), workspace.path()).unwrap();
    std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
    opts.authorizer = Arc::new(PolicyFault(policy));
    let session = Session::open(opts).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "run command and list agents"))
        .unwrap();
    let failed = settled(&session, "main").await;
    assert_eq!(AgentStatus::Interrupted, failed.status);
    assert_eq!(1, failed.usage.model_requests);
    assert_eq!(Some(100), failed.usage.input_tokens);
    session.close().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !workspace.path().join("after-close.txt").exists(),
        "owned command escaped after a runtime error"
    );
    assert!(session.events(0, 256).unwrap().iter().any(|event| matches!(&event.payload, EventPayload::ToolResult { tool_call_id, status, .. } if tool_call_id == "command" && status == "cancelled")));
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_a_polled_close_keeps_cleanup_owned_and_allows_reconnecting() {
    use std::future::Future;
    use std::task::Poll;

    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let _model = fixture(&server, |_| true, json!({"tool_calls":[tool("command", "run_command", json!({"shell":"sh","cwd":".","command":"echo started > started.txt; sleep 1; echo escaped > after-close.txt","timeout":3,"brief":"long owned command"}))]}), "tool_calls").await;
    let mut reopen = options(&server, workspace.path(), data.path());
    let session = Session::open(reopen.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "start owned work"))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !workspace.path().join("started.txt").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    {
        let close = session.close();
        tokio::pin!(close);
        std::future::poll_fn(|cx| {
            assert!(close.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    let closed = tokio::time::timeout(Duration::from_secs(3), session.close())
        .await
        .unwrap();
    let events = session.events(0, 256).unwrap();
    let result = events.iter().find_map(|event| match &event.payload {
        EventPayload::ToolResult {
            tool_call_id,
            status,
            structured,
            ..
        } if tool_call_id == "command" => Some((status, structured)),
        _ => None,
    });
    assert!(
        closed.is_ok(),
        "{closed:?}, durable command result: {result:?}"
    );
    let (status, facts) = result.expect("close must persist the command result");
    assert_eq!("cancelled", status);
    let facts = facts.as_ref().unwrap();
    assert_eq!(false, facts["cleanup_incomplete"]);
    assert_eq!(true, facts["capture_complete"]);
    assert!(!workspace.path().join("after-close.txt").exists());
    reopen.session_id = Some(session.id().into());
    let reopened = Session::open(reopen).await.unwrap();
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn request_limit_is_shared_across_agents_and_turns_and_resets_on_reopen() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = fixture(&server, |_| true, json!({"content":"accepted"}), "stop").await;
    let mut original = options(&server, workspace.path(), data.path());
    original.limits.max_requests_per_open = Some(2);
    let session = Session::open(original.clone()).await.unwrap();
    for address in ["main", "reviewer"] {
        session
            .create_agent(
                &format!("create:{address}"),
                address,
                AgentConfig::default(),
            )
            .unwrap();
        session
            .send(Input::new(
                format!("input:{address}"),
                address,
                "first turn",
            ))
            .unwrap();
        assert_eq!(AgentStatus::Idle, settled(&session, address).await.status);
    }
    session
        .send(Input::new("third", "main", "next turn"))
        .unwrap();
    let interrupted = settled(&session, "main").await;
    assert_eq!(AgentStatus::Interrupted, interrupted.status);
    assert!(interrupted
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("model request limit exhausted")));
    assert_eq!(1, interrupted.usage.model_requests);
    let steps = session.events(0, 256).unwrap().into_iter().filter(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.step_prepared")).count();
    assert_eq!(
        2, steps,
        "capacity rejection is not a dispatched model attempt"
    );
    model.assert_hits_async(2).await;
    original.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let reopened = Session::open(original).await.unwrap();
    assert_eq!(interrupted, reopened.agent("main").unwrap());
    reopened.advance("main").unwrap();
    assert_eq!(AgentStatus::Idle, settled(&reopened, "main").await.status);
    assert_eq!(2, reopened.agent("main").unwrap().usage.model_requests);
    reopened.close().await.unwrap();
    model.assert_hits_async(3).await;
}

#[tokio::test]
async fn presets_select_the_final_tools_and_explicit_model_overrides_tier_defaults() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut config = options(&server, workspace.path(), data.path());
    config.config.providers.get_mut("openai").unwrap().base_url = server.url("/selection");
    config.config.discovery.auto_discover = false;
    let model = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/selection/chat/completions")
                .matches(common::capture_http_request);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":"read only"}), "stop"));
        })
        .await;
    let session = Session::open(config).await.unwrap();
    for (address, tools) in [
        ("explore", None),
        ("explicit", Some(vec!["ask_user".into()])),
    ] {
        session
            .create_agent(
                &format!("create:{address}"),
                address,
                AgentConfig {
                    preset: Some("explore".into()),
                    tier: Some("balanced".into()),
                    model: Some(address.into()),
                    tools,
                    no_skills: true,
                    ..Default::default()
                },
            )
            .unwrap();
        session
            .send(Input::new(format!("input:{address}"), address, "inspect"))
            .unwrap();
        let state = settled(&session, address).await;
        assert_eq!(AgentStatus::Idle, state.status, "{:?}", state.last_error);
    }
    session.close().await.unwrap();
    model.assert_hits_async(2).await;
    let bodies = common::take_http_requests("/selection/chat/completions");
    assert_eq!(2, bodies.len());
    for body in bodies {
        let names = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        if body["model"] == "explore" {
            assert!(names.contains(&"read_file"));
            assert!(names.iter().all(|name| matches!(
                *name,
                "read_file"
                    | "find_files"
                    | "search_text"
                    | "fetch_url"
                    | "read_output"
                    | "list_values"
            )));
        } else {
            assert_eq!("explicit", body["model"]);
            assert_eq!(vec!["ask_user"], names);
        }
    }
}
