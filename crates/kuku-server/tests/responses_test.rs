mod common;

use common::TestServer;
use common::mock_provider::{self, text_response, tool_response};
use httpmock::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn simultaneous_question_replies_commit_once_through_durable_routing() {
    let mock = mock_provider::start_mock_provider().await;
    let asked = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .matches(|request| {
                !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                    .contains("approved-by-")
            });
        then.status(200).body(tool_response(
            "ask_user",
            "question",
            json!({"question":"Which name?"}),
        ));
    });
    let resumed = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("approved-by-");
        then.status(200).body(text_response("question answered"));
    });
    let mut server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "question-routes";
    server.submit(id, json!({"type":"send","command_id":"ask","agent":"reviewer","text":"ask for a name","create":{}})).await;
    let waiting = server
        .wait_for_snapshot(id, "reviewer", |snapshot| {
            snapshot["agent"]["status"] == "waiting"
        })
        .await;
    let request = waiting["agent"]["question"]["id"].as_str().unwrap();
    assert_eq!("reviewer", waiting["agent"]["question"]["agent"]);
    server.restart().await;
    let after_restart = server.snapshot(id, "reviewer").await;
    assert_eq!(request, after_restart["agent"]["question"]["id"]);
    assert_eq!("waiting", after_restart["agent"]["status"]);

    let left = json!({"type":"reply_question","request_id":request,"text":"approved-by-left","command_id":"left","agent":"reviewer"});
    let right = json!({"type":"reply_question","request_id":request,"text":"approved-by-right","command_id":"right","agent":"reviewer"});
    let wrong_agent = server.command(id, json!({"type":"reply_question","request_id":request,"text":"approved-by-wrong","command_id":"wrong","agent":"main"})).await;
    assert!(!wrong_agent.1["ok"].as_bool().unwrap());
    let (left_response, right_response) = tokio::join!(
        server.command(id, left.clone()),
        server.command(id, right.clone()),
    );
    let (winner, command, loser) = match (left_response.0, right_response.0) {
        (200, 409) => (left_response.1, left, right_response.1),
        (409, 200) => (right_response.1, right, left_response.1),
        unexpected => panic!("expected one winner of the reply race, got {unexpected:?}"),
    };
    assert_eq!("reply_not_pending", loser["code"]);
    let replay = server.submit(id, command.clone()).await;
    assert_eq!(true, replay["replayed"]);
    assert_eq!(winner["receipt"], replay["receipt"]);
    let mut changed = command;
    changed["text"] = json!("different answer");
    let conflicted = server.command(id, changed).await;
    assert_eq!(409, conflicted.0);
    assert_eq!("command_conflict", conflicted.1["code"]);
    server
        .wait_for_snapshot(id, "reviewer", |snapshot| {
            snapshot["agent"]["last_output"] == "question answered"
        })
        .await;
    assert!(server.snapshot(id, "reviewer").await["agent"]["question"].is_null());
    let history: Value = server
        .session_request(id, "history", &[("agent", "reviewer")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        1,
        history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["type"] == "question_reply")
            .count()
    );
    asked.assert_hits(1);
    resumed.assert_hits(1);
}

#[tokio::test]
async fn reopened_permission_applies_to_the_original_action_once() {
    let mock = mock_provider::start_mock_provider().await;
    let requested =
        mock.mock(|when, then| {
            when.method(Method::POST)
                .path("/v1/messages")
                .matches(|request| {
                    !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                        .contains("\"tool_result\"")
                });
            then.status(200).body(tool_response("write_file", "original-write", json!({
            "path":"authorized.txt","content":"authorized once\n","brief":"write after approval"
        })));
        });
    let completed = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_result\"")
            .body_contains("original-write");
        then.status(200)
            .body(text_response("authorization applied"));
    });
    let mut server = TestServer::start_deferred(mock_provider::make_test_config(mock.port())).await;
    let id = "approve-routes";
    server.submit(id, json!({"type":"send","command_id":"write","agent":"reviewer","text":"write after approval","create":{}})).await;
    let waiting = server
        .wait_for_snapshot(id, "reviewer", |snapshot| {
            snapshot["agent"]["status"] == "waiting"
        })
        .await;
    let request_id = waiting["agent"]["permission"]["id"].as_str().unwrap();
    assert_eq!("reviewer", waiting["agent"]["permission"]["conversation"]);
    let proof = server.workspace.path().join("authorized.txt");
    assert!(!proof.exists());
    server.restart().await;

    let decision = json!({"type":"reply_permission","request_id":request_id,"allow":true,"command_id":"allow","agent":"reviewer"});
    let receipt = server.submit(id, decision.clone()).await;
    let replay = server.submit(id, decision).await;
    assert_eq!(receipt["receipt"], replay["receipt"]);
    assert_eq!(true, replay["replayed"]);
    let stale = server.command(id, json!({"type":"reply_permission","request_id":request_id,"allow":false,"command_id":"stale","agent":"reviewer"})).await;
    assert_eq!(409, stale.0);
    assert_eq!("reply_not_pending", stale.1["code"]);
    server
        .wait_for_snapshot(id, "reviewer", |snapshot| {
            snapshot["agent"]["last_output"] == "authorization applied"
        })
        .await;
    assert_eq!("authorized once\n", std::fs::read_to_string(proof).unwrap());
    requested.assert_hits(1);
    completed.assert_hits(1);
}

#[tokio::test]
async fn waiting_for_reply_releases_host_execution_capacity() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("ask-for-name");
        then.status(200).body(tool_response(
            "ask_user",
            "question",
            json!({"question":"Which name?"}),
        ));
    });
    let probe = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("probe-work");
        then.status(200).body(text_response("probe completed"));
    });
    let server = TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let id = "wait-permit";
    *server.state.run_manager.lock().await = kuku_server::run_manager::RunManager::new(1);
    server
        .submit(
            id,
            json!({"type":"send","command_id":"ask","text":"ask-for-name","create":{}}),
        )
        .await;
    let waiting = server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["status"] == "waiting"
        })
        .await;
    assert!(waiting["agent"]["question"]["id"].is_string());
    server
        .submit(
            id,
            json!({"type":"create_agent","command_id":"new","agent":"probe","config":{}}),
        )
        .await;
    server
        .submit(
            id,
            json!({"type":"send","command_id":"probe","agent":"probe","text":"probe-work"}),
        )
        .await;
    server
        .wait_for_snapshot(id, "probe", |snapshot| {
            snapshot["agent"]["last_output"] == "probe completed"
        })
        .await;
    assert_eq!(
        "waiting",
        server.snapshot(id, "main").await["agent"]["status"]
    );
    probe.assert_hits(1);
}
