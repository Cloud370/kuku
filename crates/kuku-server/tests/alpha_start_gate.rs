#[allow(dead_code)]
mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use common::stream::next_json_line;
use kuku::config::{
    Config, DiscoveryConfig, HandoffConfig, LogsConfig, PluginConfig, UpdateConfig,
};
use serde_json::{json, Value};
use tokio::sync::watch;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const CORE_TASK_FIXTURE: &str = include_str!("fixtures/scenarios/core_task.json");

struct AlphaClient {
    base_url: String,
    client: wreq::Client,
}

impl AlphaClient {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: wreq::Client::new(),
        }
    }

    async fn get(&self, path: &str, authenticated: bool) -> wreq::Response {
        let request = self.client.get(format!("{}{path}", self.base_url));
        let request = if authenticated {
            request.header("authorization", format!("Bearer {TOKEN}"))
        } else {
            request
        };
        request.send().await.unwrap()
    }

    async fn post(&self, path: &str, body: &Value) -> wreq::Response {
        self.client
            .post(format!("{}{path}", self.base_url))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn json(&self, path: &str) -> Value {
        let response = self.get(path, true).await;
        assert_eq!(response.status().as_u16(), 200, "GET {path}");
        response.json().await.unwrap()
    }

    async fn post_json(&self, path: &str, body: &Value, status: u16) -> Value {
        let response = self.post(path, body).await;
        assert_eq!(response.status().as_u16(), status, "POST {path}");
        response.json().await.unwrap()
    }
}

fn core_message() -> String {
    let fixture: Value = serde_json::from_str(CORE_TASK_FIXTURE).unwrap();
    fixture["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "provider_response")
        .and_then(|event| event["text"].as_str())
        .unwrap()
        .to_owned()
}

struct ProviderState {
    requests: AtomicUsize,
    run_requested: watch::Sender<bool>,
    release: watch::Receiver<bool>,
}

struct ControlledProvider {
    addr: SocketAddr,
    run_requested: watch::Receiver<bool>,
    release: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

impl ControlledProvider {
    async fn start() -> Self {
        let (run_requested_tx, run_requested) = watch::channel(false);
        let (release, release_rx) = watch::channel(false);
        let state = Arc::new(ProviderState {
            requests: AtomicUsize::new(0),
            run_requested: run_requested_tx,
            release: release_rx,
        });
        let app = Router::new()
            .route("/v1/messages", post(provider_response))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            run_requested,
            release,
            handle,
        }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    async fn wait_for_run_request(&mut self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !*self.run_requested.borrow() {
                self.run_requested.changed().await.unwrap();
            }
        })
        .await
        .expect("runtime did not start the provider request");
    }

    fn release(&self) {
        self.release.send_replace(true);
    }
}

impl Drop for ControlledProvider {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn provider_response(State(state): State<Arc<ProviderState>>) -> impl IntoResponse {
    let request_index = state.requests.fetch_add(1, Ordering::SeqCst);
    if request_index > 0 {
        state.run_requested.send_replace(true);
        let mut release = state.release.clone();
        while !*release.borrow() {
            release.changed().await.unwrap();
        }
    }
    let body = common::mock_provider::anthropic_sse_response(json!({
        "id": "msg_alpha_gate",
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": "scenario complete"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 10}
    }));
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CONNECTION, "close"),
            (
                header::HeaderName::from_static("request-id"),
                "req_alpha_gate",
            ),
        ],
        body,
    )
}

fn unconfigured_config() -> Config {
    Config {
        tiers: std::collections::BTreeMap::new(),
        providers: std::collections::BTreeMap::new(),
        default_tier: String::new(),
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        logs: LogsConfig::default(),
        plugin: PluginConfig::default(),
        update: UpdateConfig::default(),
    }
}

struct SameHomeServer {
    base_url: String,
    handle: tokio::task::JoinHandle<()>,
}

impl SameHomeServer {
    async fn try_start(
        home: &Path,
        config: Config,
        password: Option<String>,
        registration_roots: Vec<kuku_server::platform::RegistrationRootSpec>,
    ) -> Result<Self, String> {
        let state = kuku_server::AppState::open(
            home,
            Some(config),
            password,
            registration_roots,
            "http://127.0.0.1".to_owned(),
            16,
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        let app = kuku_server::build_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| error.to_string())?;
        let addr: SocketAddr = listener.local_addr().map_err(|error| error.to_string())?;
        let handle = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            handle,
        })
    }
}

impl Drop for SameHomeServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn complete_init(
    alpha: &AlphaClient,
    server: &common::TestServer,
    provider_port: u16,
    mut revision: String,
) -> String {
    let providers = alpha
        .post_json(
            "/api/v1/init/providers",
            &json!({
                "providers": [{
                    "provider_id": "scenario",
                    "format": "anthropic",
                    "base_url": format!("http://127.0.0.1:{provider_port}"),
                    "credential": {"source": "direct_value", "value": "scenario-key"}
                }],
                "tiers": [{
                    "tier_id": "balanced",
                    "provider_id": "scenario",
                    "model": "deterministic-model",
                    "purpose": "balanced",
                    "think": null
                }],
                "expected_revision": revision
            }),
            200,
        )
        .await;
    revision = providers["server_revision"].as_str().unwrap().to_owned();

    let tier = alpha
        .post_json(
            "/api/v1/init/default-tier",
            &json!({"tier_id": "balanced", "expected_revision": revision}),
            200,
        )
        .await;
    revision = tier["server_revision"].as_str().unwrap().to_owned();

    let roots = alpha.json("/api/v1/registration-roots").await;
    let root_id = roots["items"][0]["root_id"].as_str().unwrap();
    let escape = alpha
        .post(
            "/api/v1/init/workspace",
            &json!({
                "workspace": {
                    "root_id": root_id,
                    "relative_path": "../escape",
                    "label": "Escaped workspace",
                    "expected_revision": revision
                }
            }),
        )
        .await;
    assert_eq!(escape.status().as_u16(), 400);
    let escape_error: Value = escape.json().await.unwrap();
    assert_eq!(escape_error["code"], "invalid_request");
    let after_escape = alpha.json("/api/v1/init/status").await;
    assert_eq!(after_escape["server_revision"], revision);
    let relative_path = server
        .workspace
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let workspace = alpha
        .post_json(
            "/api/v1/init/workspace",
            &json!({
                "workspace": {
                    "root_id": root_id,
                    "relative_path": relative_path,
                    "label": "Scenario workspace",
                    "expected_revision": revision
                }
            }),
            200,
        )
        .await;
    revision = workspace["server_revision"].as_str().unwrap().to_owned();

    let tested = alpha
        .post_json(
            "/api/v1/init/test",
            &json!({"tier_id": "balanced", "expected_revision": revision}),
            200,
        )
        .await;
    assert_eq!(tested["reachable"], true);

    let status = alpha.json("/api/v1/init/status").await;
    revision = status["server_revision"].as_str().unwrap().to_owned();
    let completed = alpha
        .post_json(
            "/api/v1/init/complete",
            &json!({"expected_revision": revision}),
            200,
        )
        .await;
    assert_eq!(completed["complete"], true);

    let workspaces = alpha.json("/api/v1/workspaces").await;
    workspaces["items"][0]["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn wait_for_terminal_projection(alpha: &AlphaClient, task_id: &str) -> Value {
    for _ in 0..100 {
        let projection = alpha.json(&format!("/api/v1/tasks/{task_id}")).await;
        if matches!(
            projection["task"]["state"].as_str(),
            Some("completed" | "stopped" | "failed" | "interrupted")
        ) {
            return projection;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("scenario task did not reach a terminal projection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_unconfigured_auth_init_task_disconnect_reconnect() {
    let mut provider = ControlledProvider::start().await;
    let mut server =
        common::TestServer::start_with_password(unconfigured_config(), Some(TOKEN.to_owned()))
            .await;
    let alpha = AlphaClient::new(server.base_url.clone());

    let unauthenticated = alpha.get("/api/v1/status", false).await;
    assert_eq!(unauthenticated.status().as_u16(), 401);

    let status = alpha.json("/api/v1/status").await;
    assert_eq!(status["ready"], false);
    assert_eq!(status["init"]["phase"], "required");
    let revision = status["init"]["server_revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let workspace_id = complete_init(&alpha, &server, provider.port(), revision).await;
    let registration_root = server.workspace.path().parent().unwrap().to_path_buf();
    let home = std::mem::replace(&mut server.home, tempfile::tempdir().unwrap()).keep();
    let _workspace = std::mem::replace(&mut server.workspace, tempfile::tempdir().unwrap()).keep();
    let second = SameHomeServer::try_start(
        &home,
        unconfigured_config(),
        Some(TOKEN.to_owned()),
        vec![kuku_server::platform::RegistrationRootSpec {
            label: "Test workspaces".to_owned(),
            path: registration_root.clone(),
        }],
    )
    .await;
    assert!(
        second.is_err(),
        "a second server acquired the active web home"
    );

    let first = alpha
        .post_json(
            "/api/v1/tasks",
            &json!({"workspace_id": workspace_id, "idempotency_key": "create-one"}),
            201,
        )
        .await;
    let task_id = first["projection"]["task"]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = alpha
        .post_json(
            "/api/v1/tasks",
            &json!({"workspace_id": workspace_id, "idempotency_key": "create-two"}),
            201,
        )
        .await;
    assert_ne!(task_id, second["projection"]["task"]["task_id"]);

    let page = alpha
        .json(&format!(
            "/api/v1/tasks?workspace_id={workspace_id}&search=&limit=1"
        ))
        .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let list_cursor = page["next_cursor"].as_str().unwrap();
    for path in [
        format!(
            "/api/v1/tasks?workspace_id={workspace_id}&search=different&cursor={list_cursor}&limit=1"
        ),
        format!("/api/v1/tasks/{task_id}/timeline?before={list_cursor}&limit=500"),
        format!("/api/v1/tasks/{task_id}/timeline?before=7&limit=500"),
        format!("/api/v1/tasks/{task_id}/timeline?before=malformed&limit=500"),
    ] {
        let response = alpha.get(&path, true).await;
        assert_eq!(response.status().as_u16(), 400, "GET {path}");
        let error: Value = response.json().await.unwrap();
        assert_eq!(error["code"], "invalid_request");
    }
    let first_stream = alpha
        .get(&format!("/api/v1/tasks/{task_id}/stream"), true)
        .await;
    assert_eq!(first_stream.status().as_u16(), 200);
    let mut stream = first_stream.bytes_stream();
    let mut buffer = Vec::new();
    let replacement = next_json_line(&mut stream, &mut buffer, "initial replacement").await;
    assert_eq!(replacement["event"]["type"], "projection_replaced");
    let disconnected_cursor = replacement["cursor"].as_u64().unwrap();
    drop(stream);

    let submitted = tokio::time::timeout(
        Duration::from_millis(500),
        alpha.post_json(
            &format!("/api/v1/tasks/{task_id}/runs"),
            &json!({
                "expected_task_revision": first["projection"]["task_revision"],
                "idempotency_key": "submit-one",
                "message": core_message(),
                "tier_id": "balanced",
                "skill_ids": []
            }),
            202,
        ),
    )
    .await
    .expect("submit must return 202 before the provider finishes");
    assert_ne!(submitted["run_id"], submitted["task_id"]);
    provider.wait_for_run_request().await;
    let active_while_disconnected = alpha.json(&format!("/api/v1/tasks/{task_id}")).await;
    assert!(matches!(
        active_while_disconnected["task"]["state"].as_str(),
        Some("queued" | "running" | "needs_attention" | "stopping")
    ));
    assert_eq!(
        active_while_disconnected["task"]["latest_run_id"],
        submitted["run_id"]
    );

    let resumed = alpha
        .get(
            &format!("/api/v1/tasks/{task_id}/stream?after={disconnected_cursor}"),
            true,
        )
        .await;
    assert_eq!(resumed.status().as_u16(), 200);
    let mut resumed_stream = resumed.bytes_stream();
    let mut resumed_buffer = Vec::new();
    let current = next_json_line(
        &mut resumed_stream,
        &mut resumed_buffer,
        "replacement after reconnect",
    )
    .await;
    assert_eq!(current["event"]["type"], "projection_replaced");
    let replacement_cursor = current["cursor"].as_u64().unwrap();
    assert!(replacement_cursor >= disconnected_cursor);

    provider.release();
    let delta = next_json_line(&mut resumed_stream, &mut resumed_buffer, "new live delta").await;
    assert_eq!(delta["event"]["type"], "changes_applied");
    assert!(delta["cursor"].as_u64().unwrap() > replacement_cursor);
    drop(resumed_stream);

    let ahead = alpha
        .get(
            &format!(
                "/api/v1/tasks/{task_id}/stream?after={}",
                9_007_199_254_740_991_u64
            ),
            true,
        )
        .await;
    assert_eq!(ahead.status().as_u16(), 409);
    let ahead_error: Value = ahead.json().await.unwrap();
    assert_eq!(ahead_error["code"], "cursor_ahead");

    let terminal = wait_for_terminal_projection(&alpha, &task_id).await;
    let final_stream = alpha
        .get(&format!("/api/v1/tasks/{task_id}/stream"), true)
        .await;
    let mut final_stream = final_stream.bytes_stream();
    let mut final_buffer = Vec::new();
    let final_replacement =
        next_json_line(&mut final_stream, &mut final_buffer, "terminal replacement").await;
    assert_eq!(final_replacement["event"]["projection"], terminal);
    drop(final_stream);
    drop(alpha);
    server.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let reopened = SameHomeServer::try_start(
        &home,
        unconfigured_config(),
        Some(TOKEN.to_owned()),
        vec![kuku_server::platform::RegistrationRootSpec {
            label: "Test workspaces".to_owned(),
            path: registration_root,
        }],
    )
    .await
    .unwrap();
    let reopened_alpha = AlphaClient::new(reopened.base_url.clone());
    let reopened_projection = reopened_alpha
        .json(&format!("/api/v1/tasks/{task_id}"))
        .await;
    assert_eq!(reopened_projection, terminal);
    let reopened_tasks = reopened_alpha
        .json(&format!(
            "/api/v1/tasks?workspace_id={workspace_id}&search=&limit=100"
        ))
        .await;
    let reopened_task = reopened_tasks["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["task_id"] == task_id)
        .expect("reopened task list must retain the created task");
    assert_eq!(reopened_task["title"], core_message());
    assert_ne!(reopened_task["title"], "New task");
}
