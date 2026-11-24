use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use httpmock::{Method::POST, MockServer};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub(crate) struct ControlledProvider {
    port: u16,
    calls: mpsc::Receiver<ProviderCall>,
    task: JoinHandle<()>,
}

impl ControlledProvider {
    pub(crate) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (calls, receiver) = mpsc::channel(8);
        let app = Router::new().route(
            "/v1/messages",
            post(move |Json(request): Json<Value>| {
                let calls = calls.clone();
                async move {
                    let (response, reply) = oneshot::channel();
                    calls
                        .send(ProviderCall { request, response })
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    reply
                        .await
                        .map(|body| {
                            (
                                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                                body,
                            )
                        })
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
                }
            }),
        );
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            port,
            calls: receiver,
            task,
        }
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) async fn next(&mut self) -> ProviderCall {
        tokio::time::timeout(Duration::from_secs(10), self.calls.recv())
            .await
            .expect("Provider request did not arrive")
            .expect("Provider request channel closed")
    }

    pub(crate) fn assert_idle(&mut self) {
        assert!(
            matches!(self.calls.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "Unexpected provider request"
        );
    }
}

impl Drop for ControlledProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) struct ProviderCall {
    pub(crate) request: Value,
    response: oneshot::Sender<String>,
}

impl ProviderCall {
    pub(crate) fn reply(self, body: String) {
        self.response
            .send(body)
            .expect("Provider call was already cancelled");
    }
}

pub async fn start_mock_provider() -> MockServer {
    MockServer::start_async().await
}

pub fn mock_text_response(server: &MockServer, text: &str) {
    let text = text.to_string();
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .header("request-id", "req_test")
            .header("connection", "close")
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 10}
            })));
    });
}

pub fn tool_response(tool: &str, id: &str, arguments: serde_json::Value) -> String {
    anthropic_sse_response(serde_json::json!({
        "id":"fixture-tool","type":"message","role":"assistant",
        "content":[{"type":"tool_use","id":id,"name":tool,"input":arguments}],
        "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6},
    }))
}

pub fn text_response(text: &str) -> String {
    anthropic_sse_response(serde_json::json!({
        "id":"fixture-text","type":"message","role":"assistant",
        "content":[{"type":"text","text":text}],"stop_reason":"end_turn",
        "usage":{"input_tokens":8,"output_tokens":4},
    }))
}

pub use kuku::test_support::anthropic_sse_response;

pub fn make_test_config(mock_port: u16) -> kuku::config::Config {
    use kuku::config::{
        ApiKey, Config, DiscoveryConfig, HandoffConfig, ModelConfig, ModelRef, ProviderConfig,
        ThinkLevel, UpdateConfig,
    };
    use std::collections::BTreeMap;

    let mut providers = BTreeMap::new();
    providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            options: kuku::config::ProviderOptions {
                profile: kuku::config::ProviderProfile::Compatible,
                reasoning_efforts: Some(vec![
                    "none".into(),
                    "low".into(),
                    "medium".into(),
                    "high".into(),
                ]),
                ..Default::default()
            },
            format: kuku::config::ProviderFormat::Anthropic,
            base_url: format!("http://127.0.0.1:{mock_port}"),
            api_key: ApiKey::Plaintext("test-key".to_string()),
        },
    );

    let default_model = ModelRef::new("anthropic", "claude-sonnet-4-6");
    let mut models = BTreeMap::new();
    models.insert(
        default_model.clone(),
        ModelConfig {
            request: None,
            reasoning_effort: None,
            service_tier: None,
            think: ThinkLevel::Off,
            context_window: 200_000,
            max_output_tokens: Some(48_000),
            purpose: String::new(),
            output: Default::default(),
        },
    );

    Config {
        models,
        providers,
        default_model,
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        logs: kuku::config::LogsConfig::default(),
        plugin: kuku::config::PluginConfig::default(),
        update: UpdateConfig::default(),
    }
}
