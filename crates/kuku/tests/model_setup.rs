mod common;

use std::sync::Arc;

use arc_swap::ArcSwap;
use httpmock::prelude::{GET, MockServer, POST};
use kuku::adapter::{discover_models, inspect_model};
use kuku::config::{
    ApiKey, MediaAdapterSupport, ModelConfig, RequestedOutput, load_config, parse_config_text,
};
use kuku::{AgentConfig, AgentStatus, CapabilitySource, Input, ModelRef, Session, SessionOptions};
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
default_model = {{ connection = "gateway", model = "fixture" }}
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

[model.gateway.fixture]
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
    assert!(
        inspect_model(
            &config,
            &AgentConfig {
                reasoning_effort: Some("low".into()),
                ..AgentConfig::default()
            }
        )
        .is_err()
    );

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

    let mut review = raw.model["gateway"]["fixture"].clone();
    review.purpose = Some("Independent review".into());
    review.reasoning_effort = Some("low".into());
    raw.model
        .get_mut("gateway")
        .unwrap()
        .insert("review-fixture".into(), review);
    options.config = raw.resolve().unwrap();
    let reviewed = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/gateway/infer")
                .json_body_partial(r#"{"model":"review-fixture","reasoning_effort":"low"}"#)
                .matches(|request| {
                    !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default())
                        .contains("Independent review")
                });
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
                connection: Some("gateway".into()),
                model: Some("review-fixture".into()),
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

#[test]
fn sdk_first_request_from_minimal_toml_uses_explicit_environment_references() {
    let server = MockServer::start();
    let request = server.mock(|when, then| {
        when.method(POST)
            .path("/toml/chat/completions")
            .header("authorization", "Bearer sdk-fixture-key")
            .json_body_partial(r#"{"model":"vendor/model-1.2"}"#)
            .matches(|request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
                body.get("max_tokens").is_none() && body.get("reasoning_effort").is_none()
            });
        then.header("content-type", "text/event-stream")
            .body(reply());
    });
    if !common::process::isolate_test(
        module_path!(),
        "sdk_first_request_from_minimal_toml_uses_explicit_environment_references",
        |command| {
            command
                .env("SDK_FIXTURE_KEY", "sdk-fixture-key")
                .env("SDK_FIXTURE_URL", server.url("/toml"))
                .env("SDK_FIXTURE_ID", "vendor/model-1.2")
                // These retired names must not influence the SDK's routing or authentication.
                .env("KUKU_OPENAI_MODEL", "retired-variable-model")
                .env("KUKU_OPENAI_BASE_URL", "http://127.0.0.1:1")
                .env("KUKU_OPENAI_API_KEY", "retired-variable-key");
        },
    ) {
        request.assert_hits(1);
        return;
    }
    tokio_test::block_on(async {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
default_model = { connection = "toml", model = "$SDK_FIXTURE_ID" }
[provider.toml]
format = "openai-chat"
base_url = "$SDK_FIXTURE_URL"
api_key = "$SDK_FIXTURE_KEY"
"#,
        )
        .unwrap();
        let config = load_config(&path).unwrap().resolve().unwrap();
        assert_eq!(1, config.models.len());
        let session = Session::open(SessionOptions::new(
            root.path(),
            root.path().join("data"),
            config,
        ))
        .await
        .unwrap();
        session
            .create_agent("create-main", "main", AgentConfig::default())
            .unwrap();
        send_and_wait(&session, "main", "toml-first-request").await;
        session.close().await.unwrap();
    });
}

#[tokio::test]
async fn sdk_routes_exact_pairs_and_keeps_the_session_default_after_reopen() {
    const MODEL: &str = "vendor/model-1.2";
    let server = MockServer::start_async().await;
    let first = ModelRef::new("first", MODEL);
    let second = ModelRef::new("second", MODEL);
    let mut config = common::test_config();
    let mut connection = config.providers.remove("openai").unwrap();
    config.providers.clear();
    config.models.clear();
    config.default_model = first.clone();
    let mut fixtures = Vec::new();
    for (selection, token_cap, effort) in [(&first, 2048, "low"), (&second, 4096, "medium")] {
        let api_key = format!("{}-fixture-key", selection.connection);
        let route = format!("/{}", selection.connection);
        connection.base_url = server.url(&route);
        connection.api_key = ApiKey::Plaintext(api_key.clone());
        config
            .providers
            .insert(selection.connection.clone(), connection.clone());
        config.models.insert(
            selection.clone(),
            ModelConfig {
                max_output_tokens: Some(token_cap),
                reasoning_effort: Some(effort.into()),
                ..ModelConfig::default()
            },
        );
        let body = json!({"model": MODEL, "max_tokens": token_cap, "reasoning_effort": effort})
            .to_string();
        fixtures.push(
            server
                .mock_async(move |when, then| {
                    when.method(POST)
                        .path(format!("{route}/chat/completions"))
                        .header("authorization", format!("Bearer {api_key}"))
                        .json_body_partial(body);
                    then.header("content-type", "text/event-stream")
                        .body(reply());
                })
                .await,
        );
    }

    let updates = Arc::new(ArcSwap::from_pointee(config.clone()));
    let root = tempfile::tempdir().unwrap();
    let mut options = SessionOptions::new(root.path(), root.path().join("data"), config.clone());
    options.model_config_updates = Some(updates.clone());
    let session = Session::open(options.clone()).await.unwrap();
    session
        .create_agent("create-main", "main", AgentConfig::default())
        .unwrap();
    assert_eq!(&first, session.default_model());
    send_and_wait(&session, "main", "first-request").await;

    config.default_model = second.clone();
    updates.store(Arc::new(config.clone()));
    send_and_wait(&session, "main", "after-global-default-change").await;
    session
        .configure_agent(
            "select-second",
            "main",
            AgentConfig {
                connection: Some(second.connection.clone()),
                model: Some(second.model.clone()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
    send_and_wait(&session, "main", "after-agent-selection").await;

    options.config = config.clone();
    let new_session = Session::open(options.clone()).await.unwrap();
    new_session
        .create_agent("create-main", "main", AgentConfig::default())
        .unwrap();
    assert_eq!(&second, new_session.default_model());
    send_and_wait(&new_session, "main", "new-session-uses-second").await;
    new_session.close().await.unwrap();

    options.session_id = Some(session.id().into());
    session.close().await.unwrap();
    let reopened = Session::open(options).await.unwrap();
    assert_eq!(&first, reopened.default_model());
    assert_eq!(
        Some(first.clone()),
        reopened.agent_view("main").unwrap().session_default_model
    );
    assert_eq!(
        "second",
        reopened.agent_selection("main").unwrap().connection
    );
    reopened
        .create_agent("create-other", "other", AgentConfig::default())
        .unwrap();
    send_and_wait(&reopened, "other", "reopened-session-uses-first").await;

    config.default_model = first;
    config.models.remove(&second);
    updates.store(Arc::new(config));
    assert!(
        reopened
            .agent_selection("main")
            .unwrap_err()
            .to_string()
            .contains("is not registered")
    );
    reopened
        .configure_agent("restore-session-default", "main", AgentConfig::default())
        .unwrap();
    send_and_wait(&reopened, "main", "choose-retained-first-route").await;
    reopened.close().await.unwrap();
    fixtures[0].assert_hits_async(4).await;
    fixtures[1].assert_hits_async(2).await;
}

async fn send_and_wait(session: &Session, agent: &str, id: &str) {
    session.send(Input::new(id, agent, id)).unwrap();
    let snapshot = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        session.wait_for_agent(agent),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        AgentStatus::Idle,
        snapshot.status,
        "{:?}",
        snapshot.last_error
    );
    assert_eq!(Some("Fixture reply"), snapshot.last_output.as_deref());
    let selection = session.agent_selection(agent).unwrap();
    let expected = kuku::session::runtime::ModelRequestRoute::from_selection(&selection).unwrap();
    assert!(snapshot.last_model_route.unwrap().same_route(&expected));
}
