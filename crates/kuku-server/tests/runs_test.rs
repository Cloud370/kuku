mod common;

use std::time::Duration;

use common::mock_provider::{self, text_response};
use common::TestServer;
use httpmock::Method;
use kuku::event::{EventPayload, EventStore};
use kuku::session::{AgentConfig, Input, InputSource, Session, SessionOptions};
use serde_json::{json, Value};

#[tokio::test]
async fn first_send_returns_one_durable_receipt_after_response_loss_and_restart() {
    let mock = mock_provider::start_mock_provider().await;
    let provider = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages");
        then.status(200).body(text_response("one result"));
    });
    let mut server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let fixture: Value =
        serde_json::from_str(include_str!("common/fixtures/interactive-protocol.json")).unwrap();
    let mut send = fixture["first_send"].clone();
    send["workspace"] = json!(server.workspace.path());
    let id = "first-send-lost-response";
    let unconsumed = server.command_response(id, send.clone()).await;
    assert_eq!(200, unconsumed.status());
    drop(unconsumed);
    let receipt = server
        .session_request(id, "receipts/send-example", &[])
        .send()
        .await
        .unwrap();
    assert_eq!(200, receipt.status());
    let original = receipt.json::<Value>().await.unwrap()["receipt"].clone();
    assert_eq!(
        fixture["receipt"]["receipt"]["command_id"],
        original["command_id"]
    );
    assert_eq!(fixture["receipt"]["receipt"]["agent"], original["agent"]);
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "one result"
        })
        .await;
    server.restart().await;
    let replay = server.submit(id, send.clone()).await;
    assert_eq!(true, replay["replayed"]);
    assert_eq!(original, replay["receipt"]);

    send["text"] = json!("different first intent");
    let (status, conflict) = server.command(id, send).await;
    assert_eq!(409, status);
    assert_eq!("session_creation_conflict", conflict["code"]);
    assert_eq!(1, operations(&server, id, "input_delivered"));
    provider.assert_hits(1);
}

#[tokio::test]
async fn first_send_completes_a_creation_claim_left_by_a_failed_process() {
    let mock = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&mock, "recovered");
    let config = mock_provider::make_test_config(mock.port());
    let server = TestServer::start(config.clone()).await;
    let id = "started-before-crash";
    let mut options = SessionOptions::new(server.workspace.path(), server.home.path(), config);
    options.session_id = Some(id.into());
    let mut original = Input::new("original", "main", "recover the original input");
    original.source = InputSource::Host("http".into());
    let claimed = Session::open_for_creation(options, &original, &AgentConfig::default())
        .await
        .unwrap();
    drop(claimed);
    assert_eq!(1, operations(&server, id, "session_claimed"));
    assert_eq!(0, operations(&server, id, "input_delivered"));
    let absent = server
        .session_request(id, "receipts/original", &[])
        .send()
        .await
        .unwrap();
    assert_eq!(404, absent.status());
    assert_eq!(
        "receipt_not_found",
        absent.json::<Value>().await.unwrap()["code"]
    );

    let first_line = std::fs::read(server.events_path(id))
        .unwrap()
        .split_inclusive(|byte| *byte == b'\n')
        .next()
        .unwrap()
        .len();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(server.events_path(id))
        .unwrap();
    file.set_len(first_line as u64).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let payload = json!({"type":"send","command_id":"original","text":original.text,"create":{}});
    let mut independent = payload.clone();
    independent["command_id"] = json!("independent");
    let (status, conflict) = server.command(id, independent).await;
    assert_eq!(409, status);
    assert_eq!("session_creation_conflict", conflict["code"]);
    let acknowledged = server.submit(id, payload).await;
    assert_eq!(false, acknowledged["replayed"]);
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "recovered"
        })
        .await;
    assert_eq!(1, operations(&server, id, "session_claimed"));
    assert_eq!(1, operations(&server, id, "agent_created"));
    assert_eq!(1, operations(&server, id, "input_delivered"));
}

#[tokio::test]
async fn replay_only_refuses_an_uncommitted_send_without_creating_a_session() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let response = server.request(wreq::Method::POST, "/sessions/uncommitted/commands?replay_only=true")
        .json(&json!({"type":"send","workspace":server.workspace.path(),"agent":"main","command_id":"send","text":"do not create","create":{}}))
        .send().await.unwrap();
    assert_eq!(404, response.status());
    assert_eq!(
        "receipt_not_found",
        response.json::<Value>().await.unwrap()["code"]
    );
    assert!(!server.events_path("uncommitted").exists());
}

#[tokio::test]
async fn busy_delivery_is_durable_and_rejects_conflicting_payload_recipient_timing_and_config() {
    let mock = mock_provider::start_mock_provider().await;
    let delayed = mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages");
        then.status(200)
            .delay(Duration::from_millis(900))
            .body(text_response("ready"));
    });
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    *server.state.run_manager.lock().await = kuku_server::run_manager::RunManager::new(1);
    let id = "busy-deliveries";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"first","text":"hold for deliveries","create":{}}),
        )
        .await;
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["status"] == "running"
        })
        .await;
    let queued_payload = json!({"type":"send","command_id":"queued","text":"keep for later","timing":"next_turn","config_patch":{"no_agents":true}});
    let delivery = server.submit(id, queued_payload.clone()).await;
    assert_eq!("queued", delivery["status"]);
    assert_eq!(
        delivery["receipt"],
        server.submit(id, queued_payload.clone()).await["receipt"]
    );
    for (field, change) in [
        ("text", json!("changed")),
        ("agent", json!("other")),
        ("timing", json!("steer")),
        ("config_patch", json!({"no_skills":true})),
    ] {
        let mut changed = queued_payload.clone();
        changed[field] = change;
        let (status, conflict) = server.command(id, changed).await;
        assert_eq!(409, status);
        assert_eq!("command_conflict", conflict["code"]);
    }
    let withdrawal = json!({"type":"withdraw","command_id":"withdraw","input_command_id":"queued"});
    let withdrawn = server.submit(id, withdrawal.clone()).await;
    assert_eq!("withdrawn", withdrawn["status"]);
    assert_eq!(
        withdrawn["receipt"],
        server.submit(id, withdrawal).await["receipt"]
    );
    let slots = server.state.run_manager.lock().await.active_run_ids();
    assert_eq!(1, slots.len());
    server
        .wait_for_snapshot(id, "main", |snapshot| snapshot["agent"]["status"] == "idle")
        .await;
    let accepted = server
        .submit(
            id,
            json!({"type":"withdraw","command_id":"too-late","input_command_id":"first"}),
        )
        .await;
    assert_eq!("accepted", accepted["status"]);
    assert_eq!(2, operations(&server, id, "input_delivered"));
    delayed.assert_hits(1);
}

#[tokio::test]
async fn a_late_stop_cannot_cancel_a_replacement_execution() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(Method::POST).path("/v1/messages");
        then.status(200)
            .delay(Duration::from_millis(500))
            .body(text_response("replacement finished"));
    });
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "cancel-exact-execution";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"first","text":"first work","create":{}}),
        )
        .await;
    let snapshot = server.snapshot(id, "main").await;
    let previous = snapshot["agent"]["execution_id"].as_u64().unwrap();
    let stop = json!({"type":"cancel","command_id":"old-stop","execution_id":previous});
    let cancelled = server.submit(id, stop.clone()).await;
    server
        .submit(
            id,
            json!({"type":"send","command_id":"new","text":"replacement work"}),
        )
        .await;
    let current = server.snapshot(id, "main").await;
    assert_ne!(previous, current["agent"]["execution_id"].as_u64().unwrap());
    let (status, stale) = server
        .command(
            id,
            json!({"type":"cancel","command_id":"stale","execution_id":previous}),
        )
        .await;
    assert_eq!(409, status);
    assert_eq!("execution_not_active", stale["code"]);
    let replayed = server.submit(id, stop).await;
    assert_eq!(cancelled["receipt"], replayed["receipt"]);
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "replacement finished"
        })
        .await;
}

#[tokio::test]
async fn server_rejects_relative_workspaces_and_missing_sessions() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let (status, invalid) = server
        .command(
            "session",
            json!({"type":"send","command_id":"one","text":"one","workspace":".","create":{}}),
        )
        .await;
    assert_eq!(400, status);
    assert_eq!("invalid_request", invalid["code"]);
    let (status, absent) = server
        .command(
            "session",
            json!({"type":"send","command_id":"two","text":"two"}),
        )
        .await;
    assert_eq!(404, status);
    assert_eq!("session_not_found", absent["code"]);
    assert!(!server.events_path("session").exists());
}

fn operations(server: &TestServer, session: &str, operation: &str) -> usize {
    EventStore::replay(server.events_path(session)).unwrap().iter()
        .filter(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["record"]["operation"] == operation))
        .count()
}
