use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use kuku::config::{
    ModelCapabilityDeclaration, ModelCapabilityEntry, ProviderFormat, ProviderProfile,
};
use kuku::event::EventPayload;
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{AgentConfig, AgentStatus, Input, Session, SessionOptions};
use serde_json::{Value, json};

use super::{common, until};

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg==";
const ENDPOINT: &str = "/workspace-media/responses";

#[derive(Debug)]
struct Gate;

impl ToolAuthorizer for Gate {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        let path = Path::new(action.args["path"].as_str().unwrap());
        assert_eq!("view_image", action.tool.name);
        Ok(if path == Path::new("pics/hidden.png") {
            AuthorizationDecision::Deny
        } else {
            AuthorizationDecision::Allow
        })
    }
}

#[tokio::test]
async fn authorized_image_replays_from_retained_bytes_after_reopen() {
    let server = MockServer::start_async().await;
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let png = STANDARD.decode(PNG).unwrap();
    std::fs::create_dir(work.path().join("pics")).unwrap();
    std::fs::write(work.path().join("pics/screenshot.png"), &png).unwrap();
    std::fs::write(work.path().join("pics/hidden.png"), &png).unwrap();
    std::fs::write(work.path().join("over.png"), vec![0; 1024 * 1024 + 1]).unwrap();
    std::fs::write(work.path().join("text.png"), b"not an image").unwrap();

    let tools = tool_response(&[
        ("allowed", "pics/screenshot.png"),
        ("denied", "pics/./hidden.png"),
        ("over", "over.png"),
        ("text", "text.png"),
    ]);
    let initial = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(ENDPOINT)
                .matches(initial_image_request);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(tools);
        })
        .await;
    let finish = server.mock_async(|when, then| {
        when.method(POST).path(ENDPOINT).matches(continuation);
        then.status(200).header("content-type", "text/event-stream").body(concat!(
            "event: response.output_text.delta\ndata: {\"delta\":\"image check complete\"}\n\n",
            "event: response.completed\ndata: {\"response\":{\"id\":\"media-final\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n"
        ));
    }).await;

    let mut config = common::test_config();
    let provider = config.providers.get_mut("openai").unwrap();
    provider.base_url = server.url("/workspace-media");
    provider.format = ProviderFormat::OpenAiResponses;
    provider.options.profile = ProviderProfile::DeepSeek;
    provider.options.model_capabilities.insert(
        "vision-fixture".into(),
        ModelCapabilityEntry {
            manual: Some(ModelCapabilityDeclaration {
                input_modalities: Some(vec!["text".into(), "image".into()]),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "vision-fixture".into();
    let mut options = SessionOptions::new(work.path(), data.path(), config);
    options.authorizer = Arc::new(Gate);
    options.limits.output_retention_secs = 0;

    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    assert!(
        session
            .agent_selection("main")
            .unwrap()
            .tools
            .iter()
            .any(|tool| tool.name == "view_image")
    );
    let text_only = AgentConfig {
        model: Some("text-fixture".into()),
        ..Default::default()
    };
    assert!(
        !session
            .selection_for_config("main", text_only)
            .unwrap()
            .tools
            .iter()
            .any(|tool| tool.name == "view_image")
    );
    run(&session, "first", "inspect these four images").await;
    let id = session.id().to_owned();
    let events = session.events(0, 256).unwrap();
    let results = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                status,
                ..
            } => Some((tool_call_id.as_str(), status.as_str())),
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        results,
        std::collections::BTreeMap::from([
            ("allowed", "ok"),
            ("denied", "blocked"),
            ("over", "error"),
            ("text", "error"),
        ])
    );
    session.close().await.unwrap();
    std::fs::remove_file(work.path().join("pics/screenshot.png")).unwrap();

    options.session_id = Some(id);
    let reopened = Session::open(options).await.unwrap();
    run(&reopened, "second", "use the image from the previous turn").await;
    run(&reopened, "third", "independent text-only question").await;
    reopened.close().await.unwrap();
    let journal = std::fs::read_to_string(reopened.events_path()).unwrap();
    assert!(journal.contains("model_image_saved"));
    assert!(!journal.contains(PNG));
    initial.assert_hits_async(1).await;
    finish.assert_hits_async(3).await;

    let bodies = common::take_http_requests(ENDPOINT);
    let wire_image = format!("data:image/png;base64,{PNG}");
    assert_eq!(3, bodies.len());
    for (index, body) in bodies.iter().enumerate() {
        let output = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call_output" && item["call_id"] == "allowed")
            .unwrap();
        if index < 2 {
            assert_eq!(
                output["output"][1],
                json!({"type":"input_image", "image_url":wire_image})
            );
            assert_eq!(1, body.to_string().matches("data:image/").count());
        } else {
            assert!(output["output"].is_string());
        }
    }
}

async fn run(session: &Session, id: &str, input: &str) {
    session.send(Input::new(id, "main", input)).unwrap();
    let snapshot = until(|| {
        let snapshot = session.agent("main").unwrap();
        (snapshot.status != AgentStatus::Running).then_some(snapshot)
    })
    .await;
    assert_eq!(snapshot.last_error, None);
    assert_eq!(
        snapshot.last_output.as_deref(),
        Some("image check complete")
    );
}

fn body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap()
}

fn initial_image_request(request: &HttpMockRequest) -> bool {
    let body = body(request);
    !has_tool_results(&body)
        && body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "view_image")
}

fn continuation(request: &HttpMockRequest) -> bool {
    has_tool_results(&body(request)) && common::capture_http_request(request)
}

fn has_tool_results(body: &Value) -> bool {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "function_call_output")
}

fn tool_response(calls: &[(&str, &str)]) -> String {
    let mut output = Vec::new();
    let mut stream = String::new();
    for (index, (id, path)) in calls.iter().enumerate() {
        let mut item = json!({"type":"function_call", "id":format!("item-{id}"), "call_id":id,
            "name":"view_image", "status":"in_progress", "arguments":""});
        stream.push_str(&format!(
            "event: response.output_item.added\ndata: {}\n\n",
            json!({"output_index":index, "item":item})
        ));
        let arguments = json!({"path":path}).to_string();
        stream.push_str(&format!(
            "event: response.function_call_arguments.delta\ndata: {}\n\n",
            json!({"output_index":index, "item_id":item["id"], "delta":arguments})
        ));
        item["status"] = json!("completed");
        item["arguments"] = json!(arguments);
        stream.push_str(&format!(
            "event: response.output_item.done\ndata: {}\n\n",
            json!({"output_index":index, "item":item})
        ));
        output.push(item);
    }
    stream.push_str(&format!(
        "event: response.completed\ndata: {}\n\n",
        json!({"response":{
            "id":"media-tools", "status":"completed", "output":output,
            "usage":{"input_tokens":10,"output_tokens":2}
        }})
    ));
    stream
}
