use std::sync::Arc;

use httpmock::prelude::MockServer;
use serde_json::json;

use kuku::event::EventPayload;
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{AgentConfig, AgentStatus, Input, Session};

use super::{fixture, options, settled, tool, tool_count};

#[derive(Debug)]
struct Deferred;

impl ToolAuthorizer for Deferred {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        assert_eq!("main", action.agent);
        assert_eq!("run_command", action.tool.name);
        assert_eq!("echo authorized", action.args["command"]);
        assert_eq!(
            Some(std::ffi::OsStr::new("authorized-dir")),
            action.workspace.file_name()
        );
        assert_eq!(json!(action.workspace), action.args["cwd"]);
        Ok(AuthorizationDecision::Defer)
    }
}

#[tokio::test]
async fn exact_authorization_survives_close_and_rejects_stale_replies() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("authorized-dir")).unwrap();
    let first = fixture(&server, |request| tool_count(request) == 0, json!({"tool_calls":[tool("command", "run_command", json!({"shell":if cfg!(windows) {"cmd"} else {"sh"},"cwd":"authorized-dir","command":"echo authorized","timeout":2,"brief":"check permission"}))]}), "tool_calls").await;
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
        Some(receipt.clone()),
        session
            .reply_permission_receipt("reply", "main", &request.id, true)
            .unwrap()
    );
    assert!(session
        .reply_permission_receipt("reply", "main", &request.id, false)
        .is_err());
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

#[derive(Debug)]
struct DeferCommands;

impl ToolAuthorizer for DeferCommands {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        Ok(if action.tool.name == "run_command" {
            AuthorizationDecision::Defer
        } else {
            AuthorizationDecision::Allow
        })
    }
}

#[tokio::test]
async fn question_reply_waits_in_inbox_until_prior_authorization_is_settled() {
    let task = "ask a question and run the command";
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first = fixture(&server, |request| tool_count(request) == 0, json!({"tool_calls":[tool("question", "ask_user", json!({"question":"Which color?"})),tool("command", "run_command", json!({"shell":if cfg!(windows) {"cmd"} else {"sh"},"cwd":".","command":"echo authorized","timeout":2,"brief":"run after authorization"}))]}), "tool_calls").await;
    let last = fixture(
        &server,
        |request| {
            tool_count(request) == 2
                && String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                    .contains("blue-answer")
        },
        json!({"content":"answer accepted"}),
        "stop",
    )
    .await;
    let mut original = options(&server, workspace.path(), data.path());
    original.authorizer = Arc::new(DeferCommands);
    let session = Session::open(original.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session.send(Input::new("input", "main", task)).unwrap();
    let waiting = settled(&session, "main").await;
    assert_eq!(
        AgentStatus::Waiting,
        waiting.status,
        "{:?}",
        waiting.last_error
    );
    assert_eq!(Some(task), waiting.admitted_task.as_deref());
    let question = waiting.question.unwrap();
    let permission = waiting.permission.unwrap();
    let receipt = session
        .reply_question("answer", "main", &question.id, "blue-answer")
        .unwrap();
    assert_eq!(
        Some(receipt.clone()),
        session
            .reply_question_receipt("answer", "main", &question.id, "blue-answer")
            .unwrap()
    );
    assert!(session
        .reply_question_receipt("answer", "main", &question.id, "different-answer")
        .is_err());
    assert_eq!(
        receipt,
        session
            .reply_question("answer", "main", &question.id, "blue-answer")
            .unwrap()
    );
    let queued_reply = session.agent("main").unwrap();
    assert_eq!(AgentStatus::Waiting, queued_reply.status);
    assert_eq!(1, queued_reply.pending_inputs);
    assert_eq!(None, queued_reply.queued_task);
    let mut reopen = original;
    reopen.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let session = Session::open(reopen).await.unwrap();
    let restored = session.agent("main").unwrap();
    assert_eq!(Some(task), restored.admitted_task.as_deref());
    assert_eq!(None, restored.queued_task);
    session
        .reply_permission("allow", "main", &permission.id, true)
        .unwrap();
    let done = settled(&session, "main").await;
    assert_eq!(AgentStatus::Idle, done.status, "{:?}", done.last_error);
    assert_eq!(Some("answer accepted"), done.last_output.as_deref());
    assert_eq!(Some(task), done.admitted_task.as_deref());
    assert!(session.events(0, 256).unwrap().iter().any(|event| matches!(&event.payload, EventPayload::MessageUser { text, .. } if text == "blue-answer")));
    assert!(session
        .reply_question("stale", "main", &question.id, "ignored")
        .is_err());
    session.close().await.unwrap();
    first.assert_hits_async(1).await;
    last.assert_hits_async(1).await;
}

#[tokio::test]
async fn independent_same_protocol_agents_restore_permissions_in_reverse_order() {
    let first_server = MockServer::start_async().await;
    let second_server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let first_request = fixture(&first_server, |request| tool_count(request) == 0, json!({"tool_calls":[tool("command", "run_command", json!({"shell":if cfg!(windows) {"cmd"} else {"sh"},"cwd":".","command":"echo first","timeout":2,"brief":"first independent action"}))]}), "tool_calls").await;
    let first_final = fixture(
        &first_server,
        |request| tool_count(request) == 1,
        json!({"content":"first done"}),
        "stop",
    )
    .await;
    let second_request = fixture(&second_server, |request| tool_count(request) == 0 && !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("MAIN_SECRET"), json!({"tool_calls":[tool("command", "run_command", json!({"shell":if cfg!(windows) {"cmd"} else {"sh"},"cwd":".","command":"echo second","timeout":2,"brief":"second independent action"}))]}), "tool_calls").await;
    let second_final = fixture(
        &second_server,
        |request| {
            tool_count(request) == 1
                && !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                    .contains("MAIN_SECRET")
        },
        json!({"content":"second done"}),
        "stop",
    )
    .await;
    let mut original = options(&first_server, workspace.path(), data.path());
    let mut connection = original.config.providers["openai"].clone();
    connection.base_url = second_server.url("");
    original
        .config
        .providers
        .insert("second".into(), connection);
    original.authorizer = Arc::new(DeferCommands);
    original.limits.max_model_requests = 1;
    let session = Session::open(original.clone()).await.unwrap();
    session
        .create_agent("first", "main", AgentConfig::default())
        .unwrap();
    session
        .create_agent(
            "second",
            "reviewer",
            AgentConfig {
                connection: Some("second".into()),
                ..Default::default()
            },
        )
        .unwrap();
    session
        .send(Input::new("input-a", "main", "MAIN_SECRET first task"))
        .unwrap();
    session
        .send(Input::new(
            "input-b",
            "reviewer",
            "independent selected material",
        ))
        .unwrap();
    let first = settled(&session, "main").await.permission.unwrap();
    let second = settled(&session, "reviewer").await.permission.unwrap();
    assert_ne!(first.turn, second.turn);
    let mut reopen = original;
    reopen.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let session = Session::open(reopen).await.unwrap();
    session
        .reply_permission("reply-b", "reviewer", &second.id, true)
        .unwrap();
    let second = settled(&session, "reviewer").await;
    assert_eq!(AgentStatus::Idle, second.status, "{:?}", second.last_error);
    assert_eq!(AgentStatus::Waiting, session.agent("main").unwrap().status);
    session
        .reply_permission("reply-a", "main", &first.id, true)
        .unwrap();
    let first = settled(&session, "main").await;
    assert_eq!(AgentStatus::Idle, first.status, "{:?}", first.last_error);
    session.close().await.unwrap();
    first_request.assert_hits_async(1).await;
    first_final.assert_hits_async(1).await;
    second_request.assert_hits_async(1).await;
    second_final.assert_hits_async(1).await;
}

#[tokio::test]
async fn cancelling_a_durable_wait_expires_both_question_and_authorization() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let model = fixture(&server, |_| true, json!({"tool_calls":[tool("question", "ask_user", json!({"question":"Continue?"})),tool("command", "run_command", json!({"shell":if cfg!(windows) {"cmd"} else {"sh"},"cwd":".","command":"echo never-dispatched","timeout":2,"brief":"deferred action"}))]}), "tool_calls").await;
    let mut reopen = options(&server, workspace.path(), data.path());
    reopen.authorizer = Arc::new(DeferCommands);
    let session = Session::open(reopen.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "wait for host input"))
        .unwrap();
    let waiting = settled(&session, "main").await;
    let question = waiting.question.unwrap();
    let permission = waiting.permission.unwrap();
    session.cancel("main").await.unwrap();
    assert_eq!(AgentStatus::Stopped, session.agent("main").unwrap().status);
    assert!(session
        .reply_question("late-answer", "main", &question.id, "too late")
        .is_err());
    assert!(session
        .reply_permission("late-allow", "main", &permission.id, true)
        .is_err());
    assert!(session.events(0, 256).unwrap().iter().any(|event| matches!(&event.payload, EventPayload::ToolResult { tool_call_id, status, .. } if tool_call_id == "command" && status == "cancelled")));
    reopen.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let reopened = Session::open(reopen).await.unwrap();
    assert!(reopened.agent("main").unwrap().question.is_none());
    assert!(reopened.agent("main").unwrap().permission.is_none());
    reopened.close().await.unwrap();
    model.assert_hits_async(1).await;
}
