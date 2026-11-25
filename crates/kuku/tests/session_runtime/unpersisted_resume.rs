use httpmock::prelude::{HttpMockRequest, POST};
use serde_json::{Value, json};

use kuku::event::{EventPayload, EventStore};
use kuku::{Activation, AgentConfig, AgentStatus, Input, InputStatus, Session};

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
    let mut session_options = options(&server, workspace.path(), data.path());
    let session = Session::open(session_options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input {
            activation: Activation::Enqueue,
            ..Input::new("original-input", "main", "accepted before startup failure")
        })
        .unwrap();
    session_options.session_id = Some(session.id().into());
    let events_path = session.events_path().to_path_buf();
    session.close().await.unwrap();

    // A crash after input acceptance can leave the Turn without a user message.
    {
        let mut writer = EventStore::open(&events_path).unwrap();
        for record in [
            json!({"operation":"execution_started", "address":"main"}),
            json!({
                "operation":"inputs_accepted", "address":"main", "turn":null,
                "input_ids":["original-input"], "message_input_ids":["original-input"]
            }),
        ] {
            writer
                .append(EventPayload::Unknown(json!({
                    "kind":"runtime.operation", "ts":"fixture", "record":record
                })))
                .unwrap();
        }
    }

    let session = Session::open(session_options.clone()).await.unwrap();
    let failed = session.agent("main").unwrap();
    assert_eq!(AgentStatus::Interrupted, failed.status);
    assert!(failed.last_error.as_deref().is_some_and(|error| {
        error.contains("execution interrupted before a durable settlement")
    }));
    assert_eq!(
        InputStatus::Accepted,
        session.input_status("main", "original-input").unwrap()
    );
    assert!(
        !session.events(0, 128).unwrap().iter().any(|event| matches!(
            &event.payload,
            EventPayload::MessageUser { text, .. } if text == "accepted before startup failure"
        ))
    );

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

    session.close().await.unwrap();
    let session = Session::open(session_options).await.unwrap();
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
