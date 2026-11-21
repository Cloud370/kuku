mod common;

use httpmock::prelude::{MockServer, GET, POST};
use kuku::adapter::{discover_models, inspect_model};
use kuku::config::{parse_config_text, MediaAdapterSupport, RequestedOutput};
use kuku::{AgentConfig, AgentStatus, CapabilitySource, Input, Session, SessionOptions};
use serde_json::json;

#[tokio::test]
async fn custom_endpoint_discovery_and_request_share_the_exact_resolved_destinations() {
    let server = MockServer::start_async().await;
    let listing = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/catalog/models")
                .header("authorization", "Bearer fixture-key");
            then.status(200).json_body(json!({"data":[
                {"id":"fixture"}, {"id":"other"}, {"id":"third"}
            ]}));
        })
        .await;
    let omitted = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/gateway/infer")
                .header("authorization", "Bearer fixture-key")
                .matches(|request| {
                    let body: serde_json::Value =
                        serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
                    body["model"] == "fixture"
                        && body.get("reasoning_effort").is_none()
                        && body.get("thinking").is_none()
                });
            then.header("content-type", "text/event-stream")
                .body(reply());
        })
        .await;
    let explicit = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/gateway/infer")
                .header("authorization", "Bearer fixture-key")
                .json_body_partial(
                    r#"{"model":"fixture","reasoning_effort":"low","thinking":{"type":"enabled"}}"#,
                );
            then.header("content-type", "text/event-stream")
                .body(reply());
        })
        .await;
    let mut raw = parse_config_text(&format!(
        r#"
[provider.gateway]
format = "openai-chat"
endpoint_url = {:?}
models_url = {:?}
api_key = "fixture-key"
profile = "deepseek"

[provider.gateway.model_pricing.fixture]
mode = "configured"
input = 0.2
output = 1.0
cache_read = 0.05
cache_write = 0.3

[model.only]
provider = "gateway"
model = "fixture"
think = "provider-default"
"#,
        server.url("/gateway/infer"),
        server.url("/catalog/models")
    ))
    .unwrap();

    let config = raw.resolve().unwrap();
    let models = discover_models(config.provider("gateway").unwrap())
        .await
        .unwrap();
    listing.assert_hits_async(1).await;
    assert_eq!(
        vec!["fixture", "other", "third"],
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>()
    );
    for model in models {
        raw.provider
            .get_mut("gateway")
            .unwrap()
            .options
            .model_capabilities
            .entry(model.id)
            .or_default()
            .discovered = Some(model.capabilities);
    }
    let config = raw.resolve().unwrap();
    let selection = inspect_model(&config, &AgentConfig::default()).unwrap();
    assert_eq!(server.url("/gateway/infer"), selection.request_url);
    assert_eq!(None, selection.capabilities.reasoning_source);
    assert_eq!(None, selection.capabilities.input_modalities);
    assert_eq!(None, selection.settings.reasoning_effort);
    assert!(inspect_model(
        &config,
        &AgentConfig {
            reasoning_effort: Some("low".into()),
            ..AgentConfig::default()
        }
    )
    .is_err());

    raw.provider
        .get_mut("gateway")
        .unwrap()
        .options
        .model_capabilities
        .get_mut("fixture")
        .unwrap()
        .manual = Some(kuku::config::ModelCapabilityDeclaration {
        reasoning_efforts: Some(vec!["low".into()]),
        output_modalities: Some(vec!["text".into(), "image".into()]),
        ..Default::default()
    });
    let config = raw.resolve().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut options = SessionOptions::new(workspace.path(), data.path(), config);
    let session = Session::open(options.clone()).await.unwrap();
    options.session_id = Some(session.id().into());
    session
        .create_agent("create-main", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new(
            "provider-default",
            "main",
            "Try the configured default",
        ))
        .unwrap();
    assert_eq!(
        AgentStatus::Idle,
        session.wait_for_agent("main").await.unwrap().status
    );
    omitted.assert_hits_async(1).await;
    session
        .configure_agent(
            "use-low",
            "main",
            AgentConfig {
                reasoning_effort: Some("low".into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    session
        .send(Input::new("explicit-low", "main", "Use low explicitly"))
        .unwrap();
    assert_eq!(
        AgentStatus::Idle,
        session.wait_for_agent("main").await.unwrap().status
    );
    explicit.assert_hits_async(1).await;
    let cost = session.agent("main").unwrap().usage;
    assert_eq!(44_000, cost.costs.estimated_nanodollars);
    assert_eq!(2, cost.costs.estimated_requests);
    assert_eq!(0, cost.unknown_cost_requests());
    session
        .create_agent("create-default", "child", AgentConfig::default())
        .unwrap();
    assert_eq!("fixture", session.agent_selection("child").unwrap().model);
    session
        .create_agent(
            "create-image",
            "image",
            AgentConfig {
                output: Some(RequestedOutput::Image),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    session.close().await.unwrap();

    let mut review = raw.model["only"].clone();
    review.model = "review-fixture".into();
    review.purpose = Some("Independent review".into());
    review.reasoning_effort = Some("low".into());
    raw.model.insert("review".into(), review);
    raw.default_model = Some("only".into());
    options.config = raw.resolve().unwrap();
    let reviewed = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/gateway/infer")
                .json_body_partial(r#"{"model":"review-fixture","reasoning_effort":"low"}"#)
                .body_contains("Independent review");
            then.header("content-type", "text/event-stream")
                .body(reply());
        })
        .await;
    let reopened = Session::open(options).await.unwrap();
    assert_eq!(cost.costs, reopened.agent("main").unwrap().usage.costs);
    reopened
        .create_agent(
            "create-reviewer",
            "reviewer",
            AgentConfig {
                tier: Some("review".into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    reopened
        .send(Input::new(
            "delegate-review",
            "reviewer",
            "Review the change",
        ))
        .unwrap();
    assert_eq!(
        AgentStatus::Idle,
        reopened.wait_for_agent("reviewer").await.unwrap().status
    );
    reviewed.assert_hits_async(1).await;
    let summary = reopened.agent_view("main").unwrap().session_costs();
    assert_eq!(44_000, summary.estimated_nanodollars);
    assert_eq!(1, summary.unknown_requests);
    let image = reopened.agent_selection("image").unwrap();
    assert_eq!(RequestedOutput::Image, image.output);
    assert_eq!(
        MediaAdapterSupport::TextOnly,
        image.capabilities.media_adapter
    );
    assert_eq!(
        CapabilitySource::Manual,
        image.capabilities.output_modalities.unwrap().source
    );
    reopened.close().await.unwrap();
}

fn reply() -> String {
    common::openai_sse_response(json!({
        "id":"fixture",
        "model":"fixture",
        "choices":[{"message":{"role":"assistant","content":"Fixture reply"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":100,"completion_tokens":10,"prompt_tokens_details":{"cached_tokens":60,"cache_write_tokens":10}}
    }))
}
