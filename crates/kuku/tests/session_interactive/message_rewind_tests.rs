use super::{next_call, ready, Model, Reply};
use kuku::event::{EventPayload, EventStore};
use kuku::{AgentConfig, AgentStatus, Input, ProviderFailureKind, Session, SessionOptions};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test]
async fn interrupted_rewind_survives_resume_and_accepts_new_input() {
    let root = tempfile::tempdir().unwrap();
    let (model, mut calls) = mpsc::unbounded_channel();
    let mut options = SessionOptions::new(
        root.path(),
        root.path().join("data"),
        super::common::test_config(),
    );
    options.session_id = Some("interrupted-message-rewind".into());
    options.model_adapter = Some(Arc::new(Model(model)));
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "rewound-input",
            "main",
            "do not resend this prompt",
        ))
        .unwrap();
    let first_attempt = next_call(&mut calls).await;
    assert!(first_attempt
        .request
        .body
        .to_string()
        .contains("do not resend this prompt"));
    first_attempt
        .response
        .send(Reply::TemporaryFailure(
            ProviderFailureKind::ProviderUnavailable,
        ))
        .unwrap();
    let retry = next_call(&mut calls).await;
    retry
        .response
        .send(Reply::TemporaryFailure(ProviderFailureKind::InvalidRequest))
        .unwrap();
    ready(&session, AgentStatus::Interrupted).await;
    session.close().await.unwrap();

    let reopened = Session::open(options).await.unwrap();
    let events = EventStore::replay(reopened.events_path()).unwrap();
    let target = events
        .iter()
        .find(|event| {
            matches!(&event.payload, EventPayload::MessageUser { text, .. } if text == "do not resend this prompt")
        })
        .unwrap();
    reopened
        .rewind_message(
            "rewind-interrupted-input",
            "main",
            target.id,
            "do not resend this prompt",
            false,
        )
        .unwrap();
    reopened.advance("main").unwrap();
    ready(&reopened, AgentStatus::Idle).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), calls.recv())
            .await
            .is_err()
    );

    reopened
        .send(Input::new("new-input", "main", "new prompt after rewind"))
        .unwrap();
    let next = next_call(&mut calls).await;
    let body = next.request.body.to_string();
    assert!(body.contains("new prompt after rewind"), "{body}");
    assert!(!body.contains("do not resend this prompt"), "{body}");
    next.response
        .send(Reply::Text("new input accepted"))
        .unwrap();
    ready(&reopened, AgentStatus::Idle).await;
    reopened.close().await.unwrap();
}
