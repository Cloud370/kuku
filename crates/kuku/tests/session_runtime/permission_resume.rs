use std::sync::Arc;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use serde_json::{Value, json};

use kuku::event::EventPayload;
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{AgentConfig, AgentStatus, Input, Session};

use super::{fixture, options, settled, tool, tool_count};

#[derive(Debug)]
struct Defer;

impl ToolAuthorizer for Defer {
    fn authorize(&self, _action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        Ok(AuthorizationDecision::Defer)
    }
}

fn original_input_count(request: &HttpMockRequest) -> usize {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            message["role"] == "user" && message["content"] == "permission resume input"
        })
        .count()
}

fn original_input_once(request: &HttpMockRequest) -> bool {
    original_input_count(request) == 1
}

fn original_input_without_tools(request: &HttpMockRequest) -> bool {
    original_input_once(request) && tool_count(request) == 0
}

fn original_message_events(session: &Session) -> usize {
    session
        .events(0, 256)
        .unwrap()
        .iter()
        .filter(|event| {
            matches!(&event.payload,
                EventPayload::MessageUser { text, .. } if text == "permission resume input"
            )
        })
        .count()
}

async fn resume_with_permission_reply(allow: bool) {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let failed = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/chat/completions")
                .matches(original_input_once);
            then.status(400).body("provider fixture failure");
        })
        .await;
    let mut original = options(&server, workspace.path(), data.path());
    original.authorizer = Arc::new(Defer);
    let session = Session::open(original).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "permission resume input"))
        .unwrap();

    let interrupted = settled(&session, "main").await;
    assert_eq!(AgentStatus::Interrupted, interrupted.status);
    assert_eq!(1, original_message_events(&session));
    failed.assert_hits_async(1).await;
    failed.delete_async().await;

    let action = fixture(
        &server,
        original_input_without_tools,
        json!({"tool_calls":[tool("command", "run_command", crate::common::shell_args("echo permission-resume"))]}),
        "tool_calls",
    )
    .await;
    let continuation = fixture(
        &server,
        original_input_once,
        json!({"content":"permission reply completed"}),
        "stop",
    )
    .await;

    session.advance("main").unwrap();
    let waiting = settled(&session, "main").await;
    assert_eq!(
        AgentStatus::Waiting,
        waiting.status,
        "{:?}",
        waiting.last_error
    );
    let permission = waiting.permission.unwrap();
    action.assert_hits_async(1).await;
    action.delete_async().await;
    assert_eq!(1, original_message_events(&session));

    session
        .reply_permission("permission-reply", "main", &permission.id, allow)
        .unwrap();
    let done = settled(&session, "main").await;
    assert_eq!(AgentStatus::Idle, done.status, "{:?}", done.last_error);
    assert_eq!(1, original_message_events(&session));
    continuation.assert_hits_async(1).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn approval_after_continue_reuses_the_original_input() {
    resume_with_permission_reply(true).await;
}

#[tokio::test]
async fn denial_after_continue_reuses_the_original_input() {
    resume_with_permission_reply(false).await;
}
