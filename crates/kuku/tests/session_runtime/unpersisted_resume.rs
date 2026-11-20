use httpmock::prelude::{HttpMockRequest, POST};
use serde_json::{json, Value};

use kuku::event::EventPayload;
use kuku::{Activation, AgentConfig, AgentStatus, Input, Session};

use super::{fixture, options, settled};

const COMBINED_INPUT: &str = "accepted before startup failure\n\nqueued during recovery";

fn request_has_combined_input_once(request: &HttpMockRequest) -> bool {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user" && message["content"] == COMBINED_INPUT)
        .count()
        == 1
}

fn user_messages(session: &Session) -> Vec<String> {
    session
        .events(0, 128)
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::MessageUser { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn unpersisted_accepted_input_is_kept_with_new_queued_input() {
    let server = httpmock::prelude::MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session_options = options(&server, workspace.path(), data.path());
    let session = Session::open(session_options.clone()).await.unwrap();
    session
        .create_agent(
            "create",
            "main",
            AgentConfig {
                bootstrap_skill: Some("missing-bootstrap".into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    session
        .send(Input::new(
            "original-input",
            "main",
            "accepted before startup failure",
        ))
        .unwrap();

    let failed = settled(&session, "main").await;
    assert_eq!(AgentStatus::Interrupted, failed.status);
    assert!(failed
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("bootstrap skill 'missing-bootstrap' was not found")));
    assert!(
        !session.events(0, 128).unwrap().iter().any(|event| matches!(
            &event.payload,
            EventPayload::MessageUser { text, .. } if text == "accepted before startup failure"
        ))
    );

    let skill = workspace.path().join(".fixture/skills/missing-bootstrap");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: missing-bootstrap\ndescription: Recovery fixture.\n---\nRecovery instructions.\n",
    )
    .unwrap();
    session
        .send(Input {
            activation: Activation::Enqueue,
            ..Input::new("queued-input", "main", "queued during recovery")
        })
        .unwrap();
    assert_eq!(1, session.agent("main").unwrap().pending_inputs);

    let provider_failure = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/chat/completions")
                .matches(request_has_combined_input_once);
            then.status(400).body("provider fixture failure");
        })
        .await;
    session.advance("main").unwrap();
    let interrupted = settled(&session, "main").await;
    assert_eq!(AgentStatus::Interrupted, interrupted.status);
    assert_eq!(vec![COMBINED_INPUT], user_messages(&session));
    provider_failure.assert_hits_async(1).await;
    provider_failure.delete_async().await;

    let mut reopen_options = session_options;
    reopen_options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let session = Session::open(reopen_options).await.unwrap();
    assert_eq!(
        AgentStatus::Interrupted,
        session.agent("main").unwrap().status
    );

    let success = fixture(
        &server,
        request_has_combined_input_once,
        json!({"content":"both inputs recovered"}),
        "stop",
    )
    .await;
    session.advance("main").unwrap();
    let done = settled(&session, "main").await;
    assert_eq!(AgentStatus::Idle, done.status, "{:?}", done.last_error);
    assert_eq!(Some("both inputs recovered"), done.last_output.as_deref());
    assert_eq!(vec![COMBINED_INPUT], user_messages(&session));
    success.assert_hits_async(1).await;
    session.close().await.unwrap();
}
