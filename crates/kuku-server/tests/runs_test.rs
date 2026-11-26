mod common;

use std::sync::Arc;
use std::time::Duration;

use common::TestServer;
use common::mock_provider::{self, ControlledProvider, text_response};
use httpmock::Method;
use kuku::config::ModelRef;
use kuku::event::{EventPayload, EventStore};
use kuku::session::{
    Activation, AgentConfig, Input, InputSource, InputTiming, Session, SessionOptions,
};
use serde_json::{Value, json};

#[tokio::test]
async fn session_listing_keeps_healthy_sessions_when_one_journal_is_corrupt() {
    let mock = mock_provider::start_mock_provider().await;
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let healthy = server.events_path("healthy-session");
    EventStore::open(&healthy).unwrap();
    let damaged = server.events_path("damaged-session");
    std::fs::create_dir_all(damaged.parent().unwrap()).unwrap();
    std::fs::write(&damaged, b"{this is not a valid record}\n").unwrap();

    let response = server
        .request(wreq::Method::GET, "/sessions")
        .query(&[("workspace", server.workspace.path())])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    let body = response.json::<Value>().await.unwrap();
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(1, sessions.len());
    assert_eq!("healthy-session", sessions[0]["session_id"]);
}

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
    send["create"] = json!({"reasoning_effort":"high"});
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
    let mut changed = (**server.state.config.load()).clone();
    changed
        .providers
        .get_mut("anthropic")
        .unwrap()
        .options
        .reasoning_efforts = Some(vec!["none".into()]);
    server.state.config.store(Arc::new(changed));
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
async fn invalid_first_model_leaves_no_journal_and_can_be_corrected_on_the_same_session() {
    let mock = mock_provider::start_mock_provider().await;
    let provider = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("keep this original prompt");
        then.status(200).body(text_response("corrected route"));
    });
    let config = mock_provider::make_test_config(mock.port());
    let server = TestServer::start(config).await;
    let id = "correct-first-model";
    let (status, rejected) = server
        .command(
            id,
            json!({
                "type":"send","command_id":"invalid-model","text":"keep this original prompt","create":{},
                "config_patch":{"connection":"anthropic"}
            }),
        )
        .await;
    assert_eq!(400, status);
    assert_eq!("invalid_request", rejected["code"]);
    assert!(!server.events_path(id).exists());
    let mut send = json!({
        "type":"send","command_id":"invalid-model","text":"keep this original prompt",
        "create":{"preset":"review","reasoning_effort":"unsupported"}
    });
    let (status, rejected) = server.command(id, send.clone()).await;
    assert_eq!(400, status);
    assert_eq!("model_configuration", rejected["code"]);
    assert!(!server.events_path(id).exists());
    let listed: Value = server
        .request(wreq::Method::GET, "/sessions")
        .query(&[("workspace", server.workspace.path())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json!([]), listed["sessions"]);
    let missing = server
        .session_request(id, "receipts/invalid-model", &[])
        .send()
        .await
        .unwrap();
    assert_eq!(404, missing.status());

    send["command_id"] = json!("fixed-model");
    send["create"]["reasoning_effort"] = json!("low");
    let acknowledgement = server.submit(id, send).await;
    assert_eq!("fixed-model", acknowledgement["receipt"]["command_id"]);
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "corrected route"
        })
        .await;
    assert_eq!(1, operations(&server, id, "session_claimed"));
    assert_eq!(1, operations(&server, id, "input_delivered"));
    provider.assert_hits(1);
}

#[tokio::test]
async fn compatible_model_preview_and_first_send_leave_undeclared_reasoning_unset() {
    let mock = mock_provider::start_mock_provider().await;
    let provider = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .header("x-api-key", "test-key")
            .json_body_partial(r#"{"model":"claude-haiku-4-5-20251001","stream":true}"#)
            .matches(|request| {
                serde_json::from_slice::<Value>(request.body.as_deref().unwrap_or_default())
                    .is_ok_and(|body| {
                        ["thinking", "output_config", "reasoning", "reasoning_effort"]
                            .iter()
                            .all(|key| body.get(key).is_none())
                    })
            });
        then.status(200)
            .body(text_response("compatible model result"));
    });
    let mut config = mock_provider::make_test_config(mock.port());
    config.providers.get_mut("anthropic").unwrap().options = Default::default();
    let haiku = ModelRef::new("anthropic", "claude-haiku-4-5-20251001");
    let settings = config.models[&config.default_model].clone();
    config.models.insert(haiku.clone(), settings.clone());
    config.models.insert(
        ModelRef::new("anthropic", "unavailable-settings"),
        kuku::config::ModelConfig {
            reasoning_effort: Some("none".into()),
            ..settings
        },
    );
    config.default_model = haiku;
    let server = TestServer::start(config).await;

    let response = server
        .request(wreq::Method::GET, "/models/resolve")
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    let selection = response.json::<Value>().await.unwrap()["selection"].clone();
    assert_eq!("claude-haiku-4-5-20251001", selection["model"]);
    assert_eq!(48_000, selection["max_output_tokens"]);
    assert_eq!("compatible", selection["capabilities"]["profile"]);
    assert_eq!(json!([]), selection["capabilities"]["reasoning_efforts"]);
    assert_eq!("off", selection["settings"]["requested_reasoning_effort"]);
    assert_eq!(Value::Null, selection["settings"]["reasoning_effort"]);

    let invalid = server
        .request(wreq::Method::GET, "/models/resolve")
        .query(&[
            ("connection", "anthropic"),
            ("model", "unavailable-settings"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(400, invalid.status());
    assert_eq!(
        "model_configuration",
        invalid.json::<Value>().await.unwrap()["code"]
    );
    provider.assert_hits(0);

    let id = "compatible-haiku";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"compatible-first","text":"Use the default model","create":{}}),
        )
        .await;
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "compatible model result"
        })
        .await;
    provider.assert_hits(1);
}

#[tokio::test]
async fn exact_model_pair_controls_http_requests_and_session_defaults() {
    let mut first = ControlledProvider::start().await;
    let mut second = ControlledProvider::start().await;
    let mut config = mock_provider::make_test_config(first.port());
    let original = config.default_model.clone();
    let alternate = ModelRef::new("alternate", &original.model);
    let mut provider = config.providers[&original.connection].clone();
    provider.base_url = format!("http://127.0.0.1:{}", second.port());
    config
        .providers
        .insert(alternate.connection.clone(), provider);
    config.models.insert(
        alternate.clone(),
        kuku::config::ModelConfig {
            max_output_tokens: Some(1024),
            ..config.models[&original].clone()
        },
    );
    let server = TestServer::start(config.clone()).await;
    let catalog: Value = server
        .request(wreq::Method::GET, "/models")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json!(original), catalog["default_model"]);
    assert_eq!(2, catalog["models"].as_array().unwrap().len());

    let old_session = "original-default";
    server
        .submit(
            old_session,
            json!({"type":"send","command_id":"old-first","text":"first","create":{}}),
        )
        .await;
    let call = first.next().await;
    assert_eq!(original.model, call.request["model"]);
    assert_eq!(48_000, call.request["max_tokens"]);
    call.reply(text_response("original result"));
    server
        .wait_for_snapshot(old_session, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "original result"
        })
        .await;
    second.assert_idle();

    config.default_model = alternate.clone();
    server.state.config.store(Arc::new(config));
    server
        .submit(
            old_session,
            json!({"type":"send","command_id":"old-again","text":"Use the same model"}),
        )
        .await;
    let call = first.next().await;
    assert_eq!(48_000, call.request["max_tokens"]);
    call.reply(text_response("original default retained"));
    server
        .wait_for_snapshot(old_session, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "original default retained"
        })
        .await;

    server.submit(
        "new-default",
        json!({"type":"send","command_id":"new-first","text":"Use the new default","create":{}}),
    ).await;
    let call = second.next().await;
    assert_eq!(alternate.model, call.request["model"]);
    assert_eq!(1024, call.request["max_tokens"]);
    call.reply(text_response("new default applied"));
    server
        .wait_for_snapshot("new-default", "main", |snapshot| {
            snapshot["agent"]["last_output"] == "new default applied"
        })
        .await;

    server
        .submit(
            old_session,
            json!({
                "type":"send","command_id":"old-switch","text":"Use the alternate connection",
                "config_patch":{"connection":alternate.connection,"model":alternate.model}
            }),
        )
        .await;
    let call = second.next().await;
    assert_eq!(alternate.model, call.request["model"]);
    assert_eq!(1024, call.request["max_tokens"]);
    call.reply(text_response("old agent switched"));
    server
        .wait_for_snapshot(old_session, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "old agent switched"
        })
        .await;
    first.assert_idle();
}

#[tokio::test]
async fn multiple_projects_share_one_host_and_each_agent_resolves_its_named_connection() {
    let first = mock_provider::start_mock_provider().await;
    let second = mock_provider::start_mock_provider().await;
    mock_provider::mock_text_response(&first, "first result");
    let selected = second.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("second material")
            .matches(|request| {
                !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                    .contains("first material")
            });
        then.status(200).body(text_response("second result"));
    });
    let mut config = mock_provider::make_test_config(first.port());
    let mut connection = config.providers["anthropic"].clone();
    connection.base_url = second.base_url();
    config.providers.insert("secondary".into(), connection);
    let model = config.default_model.model.clone();
    config.models.insert(
        ModelRef::new("secondary", &model),
        config.models[&config.default_model].clone(),
    );
    let server = TestServer::start(config).await;
    let workspace = tempfile::tempdir().unwrap();
    let id = "shared-service";
    server.submit(id, json!({"type":"send","command_id":"same","text":"first material","create":{"connection":"anthropic","model":model}})).await;
    server.submit(id, json!({"type":"send","command_id":"same","text":"second material","workspace":workspace.path(),
        "create":{"connection":"secondary","model":model}})).await;

    let snapshot = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "first result"
        })
        .await;
    assert_eq!("anthropic", snapshot["selection"]["connection"]);
    let second_workspace = workspace.path().canonicalize().unwrap();
    let seen = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response: Value = server
                .request(wreq::Method::GET, &format!("/sessions/{id}/snapshot"))
                .query(&[("workspace", second_workspace.to_str().unwrap())])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if response["agent"]["last_output"] == "second result" {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!("secondary", seen["selection"]["connection"]);
    assert_eq!(json!(second_workspace), seen["workspace"]);
    selected.assert_hits(1);
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
    let mut provider = ControlledProvider::start().await;
    let server = TestServer::start(mock_provider::make_test_config(provider.port())).await;
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
    let held = provider.next().await;
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
    held.reply(text_response("ready"));
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
    provider.assert_idle();
}

#[tokio::test]
async fn queued_replacement_replays_and_observes_exact_delivery_ids_after_restart() {
    let mock = mock_provider::start_mock_provider().await;
    let reply = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("edited follow-up");
        then.status(200)
            .body(text_response("edited delivery accepted"));
    });
    let config = mock_provider::make_test_config(mock.port());
    let mut server = TestServer::start(config.clone()).await;
    let id = "queued-replacement";
    let mut options = SessionOptions::new(server.workspace.path(), server.home.path(), config);
    options.session_id = Some(id.into());
    let session = Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    for command_id in ["first", "second"] {
        session
            .send(Input {
                activation: Activation::Enqueue,
                source: InputSource::Host("http".into()),
                timing: InputTiming::NextTurn,
                ..Input::new(command_id, "main", "identical follow-up")
            })
            .unwrap();
    }
    session.close().await.unwrap();
    let start = server.snapshot(id, "main").await;
    let after = start["cursor"].as_u64().unwrap().to_string();

    let replacement = json!({"type":"replace_queued", "command_id":"replacement",
        "input_command_id":"first", "text":"edited follow-up", "agent":"main", "workspace":server.workspace.path()});
    let unconsumed = server.command_response(id, replacement.clone()).await;
    assert_eq!(200, unconsumed.status());
    drop(unconsumed);
    let snapshot = server.snapshot(id, "main").await;
    assert_eq!(
        "replacement",
        snapshot["pending"][0]["receipt"]["command_id"]
    );
    assert_eq!("edited follow-up", snapshot["pending"][0]["input"]["text"]);
    assert_eq!("next_turn", snapshot["pending"][0]["input"]["timing"]);
    assert_eq!(start["pending"][1], snapshot["pending"][1]);
    let receipt = snapshot["pending"][0]["receipt"].clone();

    server.restart().await;
    let reopened = server.snapshot(id, "main").await;
    assert_eq!(snapshot["pending"], reopened["pending"]);
    for (key, status) in [("first", "withdrawn"), ("replacement", "queued")] {
        let found: Value = server
            .session_request(id, &format!("receipts/{key}"), &[])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(true, found["ok"]);
        assert_eq!(key, found["receipt"]["command_id"]);
        assert_eq!(status, found["status"]);
    }
    let replay: Value = server
        .request(wreq::Method::POST, &format!("/sessions/{id}/commands"))
        .query(&[("replay_only", true)])
        .json(&replacement)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, replay["replayed"]);
    assert_eq!(receipt, replay["receipt"]);
    assert_eq!("queued", replay["status"]);
    let mut conflicting = replacement.clone();
    conflicting["input_command_id"] = json!("second");
    let (status, conflict) = server.command(id, conflicting).await;
    assert_eq!(409, status);
    assert_eq!("command_conflict", conflict["code"]);

    server
        .submit(
            id,
            json!({"type":"withdraw", "command_id":"withdraw-second", "input_command_id":"second"}),
        )
        .await;
    server
        .submit(id, json!({"type":"advance", "command_id":"resume"}))
        .await;
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "edited delivery accepted"
        })
        .await;
    let replayed = server.submit(id, replacement).await;
    assert_eq!(receipt, replayed["receipt"]);
    assert_eq!(true, replayed["replayed"]);
    assert_eq!("accepted", replayed["status"]);
    let (status, rejected) = server
        .command(
            id,
            json!({"type":"replace_queued", "command_id":"late",
        "input_command_id":"replacement", "text":"past the admission boundary"}),
        )
        .await;
    assert_eq!(409, status);
    assert_eq!("input_not_queued", rejected["code"]);
    let missing = server
        .session_request(id, "receipts/late", &[])
        .send()
        .await
        .unwrap();
    assert_eq!(404, missing.status());
    assert_eq!(1, operations(&server, id, "input_replacement"));

    let mut stream = server
        .session_request(id, "observe", &[("agent", "main"), ("after", &after)])
        .send()
        .await
        .unwrap()
        .bytes_stream();
    let mut buffer = Vec::new();
    let deliveries = tokio::time::timeout(Duration::from_secs(10), async {
        let mut deliveries = Vec::new();
        while deliveries.len() < 3 {
            let frame =
                common::stream::next_json_line(&mut stream, &mut buffer, "queued delivery").await;
            if let Some(delivery) = frame.get("delivery") {
                deliveries.push(delivery.clone());
            }
        }
        deliveries
    })
    .await
    .unwrap();
    assert_eq!(
        vec![
            json!({"status":"replaced", "input_id":"first", "replacement_id":"replacement"}),
            json!({"status":"withdrawn", "input_id":"second"}),
            json!({"status":"accepted", "input_ids":["replacement"]}),
        ],
        deliveries
    );
    let admitted: Vec<_> = EventStore::replay(server.events_path(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::MessageUser { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(vec!["edited follow-up"], admitted);
    reply.assert_hits(1);
}

#[tokio::test]
async fn a_late_stop_cannot_cancel_a_replacement_execution() {
    let mut provider = ControlledProvider::start().await;
    let server = TestServer::start(mock_provider::make_test_config(provider.port())).await;
    let id = "cancel-exact-execution";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"first","text":"first work","create":{}}),
        )
        .await;
    let first = provider.next().await;
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
    let replacement = provider.next().await;
    drop(first);
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
    replacement.reply(text_response("replacement finished"));
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "replacement finished"
        })
        .await;
}

#[tokio::test]
async fn interrupt_wakes_the_queued_input_under_its_original_receipt_and_replays_safely() {
    let mut provider = ControlledProvider::start().await;
    let mut server = TestServer::start(mock_provider::make_test_config(provider.port())).await;
    let id = "interrupt-exact-execution";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"first","text":"first work","create":{}}),
        )
        .await;
    let first = provider.next().await;
    let previous = server.snapshot(id, "main").await["agent"]["execution_id"]
        .as_u64()
        .unwrap();
    let queued =
        json!({"type":"send","command_id":"queued","text":"queued follow-up","timing":"next_turn"});
    let original = server.submit(id, queued.clone()).await;
    assert_eq!("queued", original["status"]);
    let interrupt = json!({"type":"interrupt","command_id":"escape","execution_id":previous});
    let interrupted = server.submit(id, interrupt.clone()).await;
    let next = provider.next().await;
    drop(first);
    assert!(
        next.request["messages"]
            .to_string()
            .contains("queued follow-up")
    );
    let active = server.snapshot(id, "main").await;
    assert_ne!(previous, active["agent"]["execution_id"].as_u64().unwrap());
    assert_eq!(0, active["agent"]["pending_inputs"]);
    assert_eq!(
        interrupted["receipt"],
        server.submit(id, interrupt.clone()).await["receipt"]
    );
    let receipt = server.submit(id, queued.clone()).await;
    assert_eq!(original["receipt"], receipt["receipt"]);
    assert_eq!("accepted", receipt["status"]);
    let (status, stale) = server
        .command(
            id,
            json!({"type":"interrupt","command_id":"stale","execution_id":previous}),
        )
        .await;
    assert_eq!(409, status);
    assert_eq!("execution_not_active", stale["code"]);
    next.reply(text_response("queued follow-up completed"));
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["status"] == "idle"
                && snapshot["agent"]["last_output"] == "queued follow-up completed"
        })
        .await;
    provider.assert_idle();
    server.restart().await;
    assert_eq!(
        interrupted["receipt"],
        server.submit(id, interrupt).await["receipt"]
    );
    assert_eq!(
        original["receipt"],
        server.submit(id, queued).await["receipt"]
    );
    provider.assert_idle();
    assert_eq!(1, operations(&server, id, "agent_stopped"));
    let journal = EventStore::replay(server.events_path(id)).unwrap();
    assert_eq!(
        1,
        journal
            .iter()
            .filter(|event| matches!(&event.payload,
        EventPayload::MessageUser { text, .. } if text == "queued follow-up"))
            .count()
    );
}

#[tokio::test]
async fn interrupt_receipt_stays_acknowledged_when_a_dispatched_tool_blocks_cleanup() {
    let mut provider = ControlledProvider::start().await;
    let server = TestServer::start(mock_provider::make_test_config(provider.port())).await;
    let id = "interrupt-commits-before-cleanup";
    server
        .submit(
            id,
            json!({"type":"send","command_id":"first","text":"first","create":{}}),
        )
        .await;
    let request = provider.next().await;
    let agent = server.snapshot(id, "main").await["agent"].clone();
    let turn = agent["turn"].as_u64().unwrap();
    let execution = agent["execution_id"].as_u64().unwrap();
    let queued =
        json!({"type":"send","command_id":"queued","text":"saved follow-up","timing":"next_turn"});
    let original = server.submit(id, queued.clone()).await;
    assert_eq!("queued", original["status"]);

    // Inject the durable prefix of a dispatched side-effecting call with no
    // result. Do not execute a real command or rely on its cancellation race.
    let ts = "2026-11-25T00:00:00Z";
    let mut writer = EventStore::open(server.events_path(id)).unwrap();
    writer
        .append(EventPayload::ToolCall {
            ts: ts.into(),
            conversation: Some("main".into()),
            turn,
            tool_call_id: "unresolved".into(),
            request_id: "held".into(),
            index: 0,
            tool: "run_command".into(),
            args: json!({}),
        })
        .unwrap();
    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.tool_dispatched","ts":ts,"agent":"main",
            "turn":turn,"tool_call_id":"unresolved","tool":"run_command",
        })))
        .unwrap();
    drop(writer);

    let interrupt = json!({"type":"interrupt","command_id":"escape","execution_id":execution});
    let acknowledged = server.submit(id, interrupt.clone()).await;
    assert_eq!("escape", acknowledged["receipt"]["command_id"]);
    assert_eq!(false, acknowledged["replayed"]);
    drop(request);
    let stopped = server.snapshot(id, "main").await;
    assert_eq!("interrupted", stopped["agent"]["status"]);
    assert_eq!(true, stopped["agent"]["resume_blocked"]);
    assert!(
        stopped["agent"]["last_error"]
            .as_str()
            .unwrap()
            .contains("unresolved")
    );
    assert_eq!(1, stopped["agent"]["pending_inputs"]);
    assert_eq!(
        original["receipt"],
        server.submit(id, queued).await["receipt"]
    );
    assert_eq!(
        acknowledged["receipt"],
        server.submit(id, interrupt).await["receipt"]
    );
    let (status, _) = server
        .command(id, json!({"type":"advance","command_id":"continue"}))
        .await;
    assert!(status >= 400, "the unresolved command must block advance");
    assert_eq!(1, operations(&server, id, "agent_stopped"));
    provider.assert_idle();
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
