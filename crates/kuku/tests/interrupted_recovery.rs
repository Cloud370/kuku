mod common;

use std::time::Duration;

use kuku::event::{EventPayload, EventStore};
use kuku::{AgentConfig, AgentStatus, Input, Session, SessionOptions};
use serde_json::{Value, json};

fn append_operation(store: &mut EventStore, record: Value) {
    store
        .append(EventPayload::Unknown(json!({
            "kind": "runtime.operation",
            "ts": "2026-01-01T00:00:00Z",
            "record": record,
        })))
        .unwrap();
}

fn count_operations(session: &Session, operation: &str) -> usize {
    session
        .events(0, 256)
        .unwrap()
        .iter()
        .filter(|event| match &event.payload {
            EventPayload::Unknown(value) => {
                value["kind"] == "runtime.operation" && value["record"]["operation"] == operation
            }
            _ => false,
        })
        .count()
}

#[tokio::test]
async fn advance_after_withdrawn_unaccepted_replay_returns_idle_without_execution() {
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = SessionOptions::new(workspace.path(), data.path(), common::test_config());
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent(
            "create-main",
            "main",
            AgentConfig {
                no_skills: true,
                no_agents: true,
                ..Default::default()
            },
        )
        .unwrap();
    let session_id = session.id().to_owned();
    let events_path = session.events_path().to_path_buf();
    options.session_id = Some(session_id);
    drop(session);

    let mut store = EventStore::open(&events_path).unwrap();
    append_operation(
        &mut store,
        json!({
            "operation": "input_delivered",
            "fingerprint": "delivery-fingerprint",
            "input": Input::new("withdrawn-input", "main", "unaccepted request"),
        }),
    );
    append_operation(
        &mut store,
        json!({"operation": "execution_started", "address": "main"}),
    );
    append_operation(
        &mut store,
        json!({
            "operation": "input_withdrawal",
            "command": {
                "command_id": "withdraw-input",
                "fingerprint": "withdraw-fingerprint",
            },
            "address": "main",
            "input_command_id": "withdrawn-input",
            "status": "withdrawn",
        }),
    );
    drop(store);

    let session = Session::open(options).await.unwrap();
    let interrupted = session.agent("main").unwrap();
    assert_eq!(AgentStatus::Interrupted, interrupted.status);
    assert_eq!(0, interrupted.pending_inputs);
    assert_eq!(None, interrupted.execution_id);
    assert_eq!(0, count_operations(&session, "turn_settled"));
    let execution_starts = count_operations(&session, "execution_started");
    assert_eq!(1, execution_starts);

    let advance_session = session.clone();
    let advance = tokio::task::spawn_blocking(move || advance_session.advance("main"));
    tokio::time::timeout(Duration::from_secs(2), advance)
        .await
        .expect("Advance did not complete")
        .expect("Advance task panicked")
        .expect("Advance failed");

    let idle = session.agent("main").unwrap();
    assert_eq!(AgentStatus::Idle, idle.status);
    assert_eq!(0, idle.pending_inputs);
    assert_eq!(None, idle.execution_id);
    assert_eq!(
        execution_starts,
        count_operations(&session, "execution_started")
    );
    session.close().await.unwrap();
}
