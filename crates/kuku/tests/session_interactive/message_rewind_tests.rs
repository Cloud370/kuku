use super::{Model, Reply, next_call, ready};
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
    assert!(
        first_attempt
            .request
            .body
            .to_string()
            .contains("do not resend this prompt")
    );
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

#[tokio::test]
async fn history_switch_restores_each_branches_model_context_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let (model, mut calls) = mpsc::unbounded_channel();
    let mut options = SessionOptions::new(
        root.path(),
        root.path().join("data"),
        super::common::test_config(),
    );
    options.session_id = Some("history-jump".into());
    options.model_adapter = Some(Arc::new(Model(model)));
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    let sentinel = root.path().join("workspace.txt");
    std::fs::write(&sentinel, "current file").unwrap();
    let mut original_request = serde_json::Value::Null;
    for (id, prompt, answer) in [
        ("base", "shared past", "shared answer"),
        (
            "original",
            "original branch prompt",
            "original branch answer",
        ),
    ] {
        session.send(Input::new(id, "main", prompt)).unwrap();
        let call = next_call(&mut calls).await;
        original_request = call.request.body;
        call.response.send(Reply::Text(answer)).unwrap();
        ready(&session, AgentStatus::Idle).await;
    }
    let point = session.history_points("main", u64::MAX, 1).pop().unwrap();
    assert_eq!(2, point.number);
    assert_eq!(point.preview.as_deref(), Some("original branch prompt"));
    let original = session.events(0, 128).unwrap().into_iter().find(|event| {
        matches!(&event.payload, EventPayload::MessageUser { text, .. } if text == "original branch prompt")
    }).unwrap();
    session
        .rewind_message(
            "rewind",
            "main",
            original.id,
            "original branch prompt",
            false,
        )
        .unwrap();
    session
        .send(Input::new("alternate", "main", "alternate branch prompt"))
        .unwrap();
    let alternate = next_call(&mut calls).await;
    let alternate_request = alternate.request.body.clone();
    assert!(
        !alternate
            .request
            .body
            .to_string()
            .contains("original branch")
    );
    assert!(
        session
            .jump_history("busy", "main", point.event_id)
            .is_err()
    );
    alternate
        .response
        .send(Reply::Text("alternate branch answer"))
        .unwrap();
    ready(&session, AgentStatus::Idle).await;
    let alternate_point = session.history_points("main", u64::MAX, 1).pop().unwrap();
    let receipt = session
        .jump_history("original-head", "main", point.event_id)
        .unwrap();
    assert_eq!(
        receipt,
        session
            .jump_history("original-head", "main", point.event_id)
            .unwrap()
    );
    assert!(
        session
            .jump_history("original-head", "main", alternate_point.event_id)
            .is_err()
    );
    assert_eq!("current file", std::fs::read_to_string(&sentinel).unwrap());
    assert!(calls.try_recv().is_err());
    session.close().await.unwrap();

    let reopened = Session::open(options).await.unwrap();
    let history = kuku::session::SessionHistory::read(reopened.events_path()).unwrap();
    assert!(history.includes_cursor("main", point.event_id));
    assert!(!history.includes_cursor("main", alternate_point.event_id));
    reopened
        .send(Input::new("check-original", "main", "continue original"))
        .unwrap();
    let original_call = next_call(&mut calls).await;
    assert_wire_prefix(&original_request, &original_call.request.body);
    let body = original_call.request.body.to_string();
    assert!(body.contains("shared answer"), "{body}");
    assert!(body.contains("original branch answer"), "{body}");
    assert!(!body.contains("alternate branch"), "{body}");
    original_call
        .response
        .send(Reply::Text("original continued"))
        .unwrap();
    ready(&reopened, AgentStatus::Idle).await;

    reopened
        .jump_history("alternate-head", "main", alternate_point.event_id)
        .unwrap();
    reopened
        .send(Input::new("check-alternate", "main", "continue alternate"))
        .unwrap();
    let alternate_call = tokio::time::timeout(super::WAIT, calls.recv())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "could not continue the saved branch: {:?}",
                reopened.agent("main").unwrap().last_error
            )
        })
        .unwrap();
    assert_wire_prefix(&alternate_request, &alternate_call.request.body);
    let body = alternate_call.request.body.to_string();
    assert!(body.contains("shared answer"), "{body}");
    assert!(body.contains("alternate branch answer"), "{body}");
    assert!(!body.contains("original branch"), "{body}");
    assert!(!body.contains("original continued"), "{body}");
    alternate_call
        .response
        .send(Reply::Text("alternate continued"))
        .unwrap();
    ready(&reopened, AgentStatus::Idle).await;
    reopened.close().await.unwrap();
}

fn assert_wire_prefix(original: &serde_json::Value, body: &serde_json::Value) {
    for field in ["model", "system", "tools", "cache_control"] {
        assert_eq!(original[field], body[field], "unchanged {field}");
    }
    let prefix = original["messages"].as_array().unwrap();
    let messages = body["messages"].as_array().unwrap();
    let preserved = messages
        .get(..prefix.len())
        .expect("request keeps the committed prefix length");
    assert_eq!(
        serde_json::to_vec(prefix).unwrap(),
        serde_json::to_vec(preserved).unwrap()
    );
}
