mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post};
use axum::Router;
use kuku::session::{AgentStatus, Session, SessionOptions};
use kuku_server::server_args::ServerArgs;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use wreq::{Method, RequestBuilder};

const READY_TIMEOUT: Duration = Duration::from_secs(10);

struct Daemon {
    address: String,
    token: String,
    record: PathBuf,
    task: JoinHandle<()>,
}

impl Daemon {
    async fn start(home: &Path, config: &kuku::config::Config, web: Option<Router>) -> Self {
        let mut config = config.clone();
        for tier in ["strong", "light"] {
            config
                .tiers
                .insert(tier.into(), config.tiers["balanced"].clone());
        }
        let content = config.redacted_display().replace(
            "[provider.anthropic]",
            "[provider.anthropic]\nprofile = \"compatible\"\nreasoning_efforts = [\"none\"]",
        );
        fs::write(
            home.join("config.toml"),
            format!("{content}\n[discovery]\nauto_discover = false\n[plugin]\nenabled = false\n"),
        )
        .unwrap();
        let args = args(home);
        let task = tokio::spawn(async move {
            kuku_server::service::serve(args, web).await.unwrap();
        });
        let record = home.join("service").join("connection.json");
        let found: Value = tokio::time::timeout(READY_TIMEOUT, async {
            loop {
                if let Ok(bytes) = fs::read(&record) {
                    return serde_json::from_slice(&bytes).unwrap();
                }
                assert!(
                    !task.is_finished(),
                    "service exited before publishing discovery"
                );
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .expect("managed service did not publish discovery");
        Self {
            address: format!("http://{}", found["address"].as_str().unwrap()),
            token: found["token"].as_str().unwrap().to_owned(),
            record,
            task,
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        wreq::Client::builder()
            .no_proxy()
            .timeout(READY_TIMEOUT)
            .build()
            .unwrap()
            .request(method, format!("{}{path}", self.address))
    }

    fn native(&self, method: Method, path: &str) -> RequestBuilder {
        self.request(method, path).bearer_auth(&self.token)
    }

    async fn wait_for_stop(&mut self) {
        tokio::time::timeout(READY_TIMEOUT, &mut self.task)
            .await
            .expect("server should release its listener")
            .unwrap();
        assert!(!self.record.exists());
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn discovery_and_browser_handoff_are_bound_to_one_authenticated_instance() {
    let home = tempfile::tempdir().unwrap();
    let config = common::mock_provider::make_test_config(9);
    let assets = Router::new().route("/", get(|| async { "embedded UI" }));
    let mut daemon = Daemon::start(home.path(), &config, Some(assets)).await;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for path in [daemon.record.parent().unwrap(), &daemon.record] {
            assert_eq!(0, fs::metadata(path).unwrap().permissions().mode() & 0o077);
        }
    }
    let duplicate = kuku_server::service::serve(args(home.path()), None)
        .await
        .unwrap_err();
    assert!(duplicate.to_string().contains("already owns"));
    let assets = daemon.request(Method::GET, "/").send().await.unwrap();
    assert_eq!("embedded UI", assets.text().await.unwrap());
    assert_eq!(
        401,
        daemon
            .request(Method::GET, "/service/status")
            .send()
            .await
            .unwrap()
            .status()
    );
    let native = daemon
        .native(Method::GET, "/service/status")
        .send()
        .await
        .unwrap();
    let status: Value = native.json().await.unwrap();
    assert_eq!(std::process::id(), status["pid"]);
    assert_eq!(false, status["draining"]);

    let ticket: Value = daemon
        .native(Method::POST, "/service/browser-ticket")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let exchange = |origin: &str| {
        daemon
            .request(Method::POST, "/service/browser-session")
            .header("Origin", origin)
            .json(&ticket)
    };
    assert_eq!(
        403,
        exchange("http://unrelated.test")
            .send()
            .await
            .unwrap()
            .status()
    );
    let response = exchange(&daemon.address).send().await.unwrap();
    assert_eq!(200, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    let handoff: Value = response.json().await.unwrap();
    let browser_token = handoff["token"].as_str().unwrap();
    assert_ne!(&daemon.token, browser_token);
    assert_eq!(
        401,
        exchange(&daemon.address).send().await.unwrap().status()
    );
    assert_eq!(
        200,
        daemon
            .request(Method::GET, "/service/status")
            .bearer_auth(browser_token)
            .send()
            .await
            .unwrap()
            .status()
    );
    assert_eq!(
        403,
        daemon
            .request(Method::POST, "/service/restart")
            .bearer_auth(browser_token)
            .send()
            .await
            .unwrap()
            .status()
    );
    assert_eq!(
        202,
        daemon
            .native(Method::POST, "/service/restart")
            .send()
            .await
            .unwrap()
            .status()
    );
    daemon.wait_for_stop().await;
}

#[tokio::test]
async fn restart_defers_during_execution_and_releases_a_durable_wait() {
    let provider_release = Arc::new(Notify::new());
    let reached = Arc::new(Notify::new());
    let provider = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = common::mock_provider::make_test_config(provider.local_addr().unwrap().port());
    let provider_router = Router::new().route(
        "/v1/messages",
        post({
            let gate = provider_release.clone();
            let reached = reached.clone();
            move || async move {
                reached.notify_one();
                gate.notified().await;
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    common::mock_provider::anthropic_sse_response(json!({
                        "id":"ask","type":"message","role":"assistant",
                        "content":[{"type":"tool_use","id":"ask-name","name":"ask_user",
                            "input":{"question":"Which name?"}}],
                        "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6}
                    })),
                )
            }
        }),
    );
    let provider_task = tokio::spawn(async move {
        axum::serve(provider, provider_router).await.unwrap();
    });
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let canonical = workspace.path().canonicalize().unwrap();
    let mut daemon = Daemon::start(home.path(), &config, None).await;
    let response = daemon
        .native(Method::POST, "/sessions/restart-wait/commands")
        .json(&json!({
            "workspace": canonical,
            "agent": "main",
            "command_id": "first",
            "type": "send",
            "text": "ask for a name",
            "create": {}
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let response: Value = response.json().await.unwrap();
    assert_eq!(200, status, "{response}");
    tokio::time::timeout(READY_TIMEOUT, reached.notified())
        .await
        .unwrap();

    let reply = daemon
        .native(Method::POST, "/service/restart")
        .send()
        .await
        .unwrap();
    assert_eq!(409, reply.status());
    let problem: Value = reply.json().await.unwrap();
    assert_eq!("service_busy", problem["code"]);
    assert!(problem["busy"][0]
        .as_str()
        .unwrap()
        .starts_with("restart-wait/main/"));
    assert!(!daemon.task.is_finished());
    provider_release.notify_one();

    let home = home.path().canonicalize().unwrap();
    let log = kuku::session::session_events_path(&home, &canonical, "restart-wait").unwrap();
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            if Session::read_agents(&log).is_ok_and(|agents| {
                agents
                    .iter()
                    .any(|agent| agent.status == AgentStatus::Waiting)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a durable question can wait across a service restart");
    let reply = daemon
        .native(Method::POST, "/service/restart")
        .send()
        .await
        .unwrap();
    let status = reply.status();
    assert_eq!(202, status, "{}", reply.text().await.unwrap());
    let journal = fs::File::open(&log).unwrap();
    journal
        .try_lock_shared()
        .expect("accepted restart must release the exclusive Session writer");
    drop(journal);
    daemon.wait_for_stop().await;

    let mut options = SessionOptions::new(&canonical, &home, config);
    options.session_id = Some("restart-wait".into());
    let reopened = Session::open(options).await.unwrap();
    let agent = reopened.agent("main").unwrap();
    assert_eq!(AgentStatus::Waiting, agent.status);
    assert_eq!("Which name?", agent.question.unwrap().text);
    reopened.close().await.unwrap();
    provider_task.abort();
}

fn args(home: &Path) -> ServerArgs {
    ServerArgs {
        listen: "127.0.0.1:0".into(),
        config: None,
        data_dir: Some(home.to_path_buf()),
        password: None,
        max_concurrent_runs: 1,
        managed: true,
    }
}
