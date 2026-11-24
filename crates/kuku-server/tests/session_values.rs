mod common;

use common::TestServer;
use common::mock_provider::{self, text_response};
use httpmock::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn saved_values_reopen_and_dispatch_the_selected_version() {
    let provider = mock_provider::start_mock_provider().await;
    let mut server = TestServer::start(mock_provider::make_test_config(provider.port())).await;
    let id = "values-before-input";
    let original_text = "original paste contents";
    let (code, original) = save(&server, id, original_text, false, true).await;
    assert_eq!(200, code, "{original}");
    assert_eq!("baseline", original["name"]);
    assert_eq!("text", original["kind"]);
    assert_eq!(
        original_text.len(),
        original["reference"]["bytes"].as_u64().unwrap() as usize
    );
    let reference = original["reference"].clone();
    let output_id = reference["id"].as_str().unwrap();
    let run = provider.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains(output_id)
            .matches(|request| {
                !request.body.as_ref().is_some_and(|body| {
                    String::from_utf8_lossy(body).contains("original paste contents")
                })
            });
        then.status(200).body(text_response("selected version"));
    });
    assert!(server.snapshot(id, "main").await["agent"]["turn"].is_null());
    run.assert_hits(0);

    server.restart().await;
    let named = server
        .session_request(id, "values", &[("agent", "main")])
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(json!([original]), named["values"]);
    let path = format!("outputs/{output_id}");
    let output = server
        .session_request(id, &path, &[("agent", "main")])
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(original_text, output["output"]["segments"][0]["text"]);
    server
        .submit(
            id,
            json!({"type":"create_agent","agent":"reviewer","command_id":"create-reviewer"}),
        )
        .await;
    let private = server
        .session_request(id, &path, &[("agent", "reviewer")])
        .send()
        .await
        .unwrap();
    assert_eq!(404, private.status());
    assert_eq!(
        "output_unavailable",
        private.json::<Value>().await.unwrap()["code"]
    );
    assert_eq!(
        400,
        save(&server, id, "changed binding", false, false).await.0
    );
    let (code, replacement) = save(&server, id, "changed binding", true, false).await;
    assert_eq!(200, code, "{replacement}");
    assert_ne!(reference, replacement["reference"]);

    let mut input = json!({
        "type":"send","command_id":"attach-version","text":"Use the selected paste",
        "values":[reference],
    });
    let receipt = server.submit(id, input.clone()).await["receipt"].clone();
    server
        .wait_for_snapshot(id, "main", |snapshot| {
            snapshot["agent"]["last_output"] == "selected version"
        })
        .await;
    let replay = server.submit(id, input.clone()).await;
    assert_eq!(true, replay["replayed"]);
    assert_eq!(receipt, replay["receipt"]);
    input["values"] = json!([replacement["reference"]]);
    assert_eq!(409, server.command(id, input).await.0);
    run.assert_hits(1);
}

async fn save(
    server: &TestServer,
    id: &str,
    text: &str,
    replace: bool,
    create: bool,
) -> (u16, Value) {
    let response = server
        .request(
            wreq::Method::PUT,
            &format!("/sessions/{id}/values/baseline"),
        )
        .json(&json!({
            "workspace":server.workspace.path(),
            "agent":"main",
            "source":{"kind":"text","value":text},
            "replace":replace,
            "create":create.then(|| json!({})),
        }))
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}
