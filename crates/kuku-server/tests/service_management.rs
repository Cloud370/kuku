mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use common::mock_provider::{ControlledProvider, tool_response};
use kuku::session::{AgentStatus, Session, SessionOptions};
use kuku_server::server_args::ServerArgs;
use serde_json::{Value, json};
use sha2::Digest;
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
        let provider = &config.providers["anthropic"];
        let model = &config.default_model.model;
        let content = format!(
            r#"
default_model = {{ connection = "anthropic", model = {model:?} }}

[provider.anthropic]
format = "anthropic"
base_url = {:?}
api_key = "test-key"
profile = "compatible"
reasoning_efforts = ["none"]

[model.anthropic.{model:?}]
think = "off"
max_output_tokens = 48000

[discovery]
auto_discover = false
"#,
            provider.base_url
        );
        fs::write(home.join("config.toml"), content).unwrap();
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
async fn model_default_plan_is_authenticated_hash_bound_and_has_no_path_input() {
    let home = tempfile::tempdir().unwrap();
    let config = common::mock_provider::make_test_config(9);
    let daemon = Daemon::start(home.path(), &config, None).await;
    let path = home.path().join("config.toml");
    let mut raw = fs::read_to_string(&path).unwrap();
    raw.push_str("\n# safe-plan-secret-marker\n");
    fs::write(&path, &raw).unwrap();
    let digest = format!("{:x}", sha2::Sha256::digest(raw.as_bytes()));
    let model = &config.default_model.model;
    let route = format!(
        "/service/model-default-plan?connection=anthropic&model={model}&config_sha256={digest}"
    );

    let unauthorized = daemon.request(Method::GET, &route).send().await.unwrap();
    assert_eq!(401, unauthorized.status());

    let response = daemon.native(Method::GET, &route).send().await.unwrap();
    assert_eq!(200, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    let plan: Value = response.json().await.unwrap();
    let mut fields = plan
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    fields.sort();
    assert_eq!(["already_default", "config_sha256"], fields.as_slice());
    assert!(!plan.to_string().contains("safe-plan-secret-marker"));

    let mismatch = route.replace(&digest, &"0".repeat(64));
    let response = daemon.native(Method::GET, &mismatch).send().await.unwrap();
    assert_eq!(409, response.status());
    let error: Value = response.json().await.unwrap();
    assert_eq!("config_changed", error["code"]);
    assert!(!error.to_string().contains("safe-plan-secret-marker"));

    let arbitrary_path = format!("{route}&config_path=%2Ftmp%2Fother.toml");
    assert_eq!(
        400,
        daemon
            .native(Method::GET, &arbitrary_path)
            .send()
            .await
            .unwrap()
            .status()
    );
}

#[tokio::test]
async fn status_reports_the_configuration_loaded_by_the_running_service() {
    let home = tempfile::tempdir().unwrap();
    let config = common::mock_provider::make_test_config(9);
    let daemon = Daemon::start(home.path(), &config, None).await;
    let path = home.path().join("config.toml");
    let original = fs::read_to_string(&path).unwrap();
    let original_hash = kuku::config::config_sha256(original.as_bytes());
    let status = async || {
        daemon
            .native(Method::GET, "/service/status")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    assert_eq!(original_hash, status().await["config_sha256"]);

    let updated = original.replace("48000", "4096");
    let updated_hash = kuku::config::config_sha256(updated.as_bytes());
    fs::write(&path, updated).unwrap();
    tokio::time::timeout(READY_TIMEOUT, async {
        while status().await["config_sha256"] != updated_hash {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("service did not publish the successfully loaded config hash");
    let resolved = daemon
        .native(Method::GET, "/models/resolve")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(4096, resolved["selection"]["max_output_tokens"]);
}

#[tokio::test]
async fn restart_defers_during_execution_and_releases_a_durable_wait() {
    let mut provider = ControlledProvider::start().await;
    let config = common::mock_provider::make_test_config(provider.port());
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
    let call = provider.next().await;

    let reply = daemon
        .native(Method::POST, "/service/restart")
        .send()
        .await
        .unwrap();
    assert_eq!(409, reply.status());
    let problem: Value = reply.json().await.unwrap();
    assert_eq!("service_busy", problem["code"]);
    assert!(
        problem["busy"][0]
            .as_str()
            .unwrap()
            .starts_with("restart-wait/main/")
    );
    assert!(!daemon.task.is_finished());
    call.reply(tool_response(
        "ask_user",
        "ask-name",
        json!({"question":"Which name?"}),
    ));

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
    provider.assert_idle();
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
