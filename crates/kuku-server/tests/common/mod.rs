#[allow(
    dead_code,
    reason = "integration binaries use different subsets of the mock provider fixture"
)]
pub mod mock_provider;
#[allow(
    dead_code,
    reason = "integration binaries use different subsets of stream helpers"
)]
pub mod stream;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[allow(
    dead_code,
    reason = "integration binaries inspect different subsets of server state"
)]
pub struct TestServer {
    pub addr: SocketAddr,
    pub base_url: String,
    pub workspace: tempfile::TempDir,
    pub home: tempfile::TempDir,
    pub state: Arc<kuku_server::AppState>,
    pub deadline: tokio::time::Instant,
    client: wreq::Client,
    handle: Option<tokio::task::JoinHandle<()>>,
}

#[allow(
    dead_code,
    reason = "integration binaries use different subsets of shared helpers"
)]
impl TestServer {
    const TOKEN: &str = "server-contract-test-token";

    pub async fn start(config: kuku::config::Config) -> Self {
        Self::start_with_password(config, Some(Self::TOKEN.into())).await
    }

    pub async fn start_with_password(
        config: kuku::config::Config,
        password: Option<String>,
    ) -> Self {
        Self::start_with_authorizer(config, password, Arc::new(kuku::permission::TrustedLocal))
            .await
    }

    pub async fn start_deferred(config: kuku::config::Config) -> Self {
        Self::start_with_authorizer(config, Some(Self::TOKEN.into()), Arc::new(DeferTools)).await
    }

    pub async fn start_with_authorizer(
        config: kuku::config::Config,
        password: Option<String>,
        authorizer: Arc<dyn kuku::permission::ToolAuthorizer>,
    ) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        let config_store = Arc::new(ArcSwap::from_pointee(config));

        let state = Arc::new(kuku_server::AppState {
            run_manager: Mutex::new(kuku_server::run_manager::RunManager::new(16)),
            config: config_store,
            password,
            kuku_home: home.path().to_path_buf(),
            managed_cli: None,
            authorizer,
        });

        let (addr, handle) = listen(state.clone()).await;
        let base_url = format!("http://{addr}");

        Self {
            addr,
            base_url,
            workspace,
            home,
            state,
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            client: wreq::Client::builder().no_proxy().build().unwrap(),
            handle: Some(handle),
        }
    }

    pub async fn restart(&mut self) {
        self.close().await;
        let state = Arc::new(kuku_server::AppState {
            run_manager: Mutex::new(kuku_server::run_manager::RunManager::new(16)),
            config: self.state.config.clone(),
            password: self.state.password.clone(),
            kuku_home: self.home.path().into(),
            managed_cli: self.state.managed_cli.clone(),
            authorizer: self.state.authorizer.clone(),
        });
        let (addr, handle) = listen(state.clone()).await;
        self.addr = addr;
        self.base_url = format!("http://{addr}");
        self.state = state;
        self.handle = Some(handle);
    }

    pub async fn close(&mut self) {
        self.state
            .run_manager
            .lock()
            .await
            .close_all()
            .await
            .unwrap();
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    pub fn request(&self, method: wreq::Method, path: &str) -> wreq::RequestBuilder {
        let request = self
            .client
            .request(method, format!("{}{path}", self.base_url));
        match &self.state.password {
            Some(password) => request.bearer_auth(password),
            None => request,
        }
    }

    pub fn session_request(
        &self,
        id: &str,
        route: &str,
        parameters: &[(&str, &str)],
    ) -> wreq::RequestBuilder {
        let mut query = vec![("workspace", self.workspace.path().to_str().unwrap())];
        query.extend_from_slice(parameters);
        self.request(wreq::Method::GET, &format!("/sessions/{id}/{route}"))
            .query(&query)
    }

    pub async fn command_response(&self, id: &str, mut body: Value) -> wreq::Response {
        if body["workspace"].is_null() {
            body["workspace"] = json!(self.workspace.path());
        }
        if body["agent"].is_null() {
            body["agent"] = json!("main");
        }
        self.request(wreq::Method::POST, &format!("/sessions/{id}/commands"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    pub async fn command(&self, id: &str, body: Value) -> (u16, Value) {
        let response = self.command_response(id, body).await;
        let status = response.status().as_u16();
        (status, response.json().await.unwrap())
    }

    pub async fn submit(&self, id: &str, body: Value) -> Value {
        let (status, value) = self.command(id, body).await;
        assert_eq!(200, status, "{value}");
        assert_eq!(true, value["ok"], "{value}");
        value
    }

    pub async fn snapshot(&self, id: &str, agent: &str) -> Value {
        let response = self
            .session_request(id, "snapshot", &[("agent", agent)])
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let value: Value = response.json().await.unwrap();
        assert_eq!(200, status, "{value}");
        value
    }

    pub async fn wait_for_snapshot(
        &self,
        id: &str,
        agent: &str,
        ready: impl Fn(&Value) -> bool,
    ) -> Value {
        let mut last_snapshot = None;
        let result = tokio::time::timeout_at(self.deadline, async {
            loop {
                let snapshot = self.snapshot(id, agent).await;
                if ready(&snapshot) {
                    return snapshot;
                }
                last_snapshot = Some(snapshot);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        result.unwrap_or_else(|_| {
            let summary = last_snapshot.as_ref().map(snapshot_summary);
            panic!("Session '{id}', Agent '{agent}' did not reach the expected state before the fixture deadline; last snapshot: {summary:?}")
        })
    }

    pub fn events_path(&self, id: &str) -> std::path::PathBuf {
        kuku::session::session_events_path(self.home.path(), self.workspace.path(), id).unwrap()
    }
}

async fn listen(state: Arc<kuku_server::AppState>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let app = kuku_server::build_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, handle)
}

pub fn snapshot_summary(snapshot: &Value) -> Value {
    let agent = &snapshot["agent"];
    json!({
        "cursor": snapshot["cursor"],
        "status": agent["status"],
        "execution_id": agent["execution_id"],
        "turn": agent["turn"],
        "pending_inputs": agent["pending_inputs"],
        "phase": snapshot["progress"]["phase"],
        "error": agent["last_error"].as_str().map(|error| error.chars().take(512).collect::<String>()),
    })
}

#[derive(Debug)]
#[allow(
    dead_code,
    reason = "only deferred-authorization integration cases construct this policy"
)]
struct DeferTools;

impl kuku::permission::ToolAuthorizer for DeferTools {
    fn authorize(
        &self,
        _action: &kuku::permission::ToolAuthorization,
    ) -> kuku::Result<kuku::permission::AuthorizationDecision> {
        Ok(kuku::permission::AuthorizationDecision::Defer)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}
