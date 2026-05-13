use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use httpmock::prelude::*;
use kuku::event::{EventPayload, EventStore};
use kuku::session::session_events_path;
use kuku::{query, Error, Provider};
use tempfile::TempDir;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct TestEnv {
    _guard: MutexGuard<'static, ()>,
    home: TempDir,
    workspace: TempDir,
    previous_kuku_home: Option<OsString>,
    previous_cwd: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let guard = env_lock().lock().unwrap();
        let previous_kuku_home = std::env::var_os("KUKU_HOME");
        let previous_cwd = std::env::current_dir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();

        std::env::set_var("KUKU_HOME", home.path());
        std::env::set_current_dir(workspace.path()).unwrap();

        Self {
            _guard: guard,
            home,
            workspace,
            previous_kuku_home,
            previous_cwd,
        }
    }

    fn events_path(&self, session_id: &str) -> PathBuf {
        let workspace = std::fs::canonicalize(self.workspace.path()).unwrap();
        session_events_path(self.home.path(), &workspace, session_id).unwrap()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.previous_cwd).unwrap();
        match &self.previous_kuku_home {
            Some(value) => std::env::set_var("KUKU_HOME", value),
            None => std::env::remove_var("KUKU_HOME"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_success_returns_text_and_writes_events() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .header("x-api-key", "test-key")
            .header("anthropic-version", "2023-06-01");
        then.status(200)
            .header("request-id", "req_abc")
            .json_body(serde_json::json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Hello from Claude!"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 10}
            }));
    });

    let output = query("say hello")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .run()
        .await
        .unwrap();

    mock.assert();
    assert_eq!(output.text, "Hello from Claude!");

    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert_eq!(events.len(), 6);
    assert!(matches!(
        events[3].payload,
        EventPayload::ModelRequest { .. }
    ));
    assert!(matches!(
        events[4].payload,
        EventPayload::ModelResponse { .. }
    ));
    assert!(matches!(events[5].payload, EventPayload::TurnEnd { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn openai_success_returns_text_and_writes_events() {
    let env = TestEnv::new();
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/chat/completions")
            .header("authorization", "Bearer openai-key");
        then.status(200).json_body(serde_json::json!({
            "choices": [{"message": {"content": "Hi from GPT!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4}
        }));
    });

    let output = query("hi")
        .provider(Provider::OpenAiCompatible)
        .model("gpt-5.4-mini")
        .base_url(server.base_url())
        .api_key("openai-key")
        .run()
        .await
        .unwrap();

    mock.assert();
    assert_eq!(output.text, "Hi from GPT!");
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    assert_eq!(events.len(), 6);
}

#[tokio::test(flavor = "current_thread")]
async fn http_error_writes_model_error_and_turn_end() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let sid = "s_http_err";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(401)
            .header("request-id", "req_http_error")
            .body("unauthorized");
    });

    let err = query("test")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("bad")
        .session(sid)
        .run()
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Provider(_)));

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::TurnEnd { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn missing_config_writes_error_without_fake_model_request() {
    let env = TestEnv::new();
    let sid = "s_no_cfg";
    let saved_provider = std::env::var_os("KUKU_PROVIDER");
    let saved_anthropic_key = std::env::var_os("KUKU_ANTHROPIC_API_KEY");
    let saved_openai_key = std::env::var_os("KUKU_OPENAI_API_KEY");
    let saved_key = std::env::var_os("KUKU_API_KEY");

    std::env::remove_var("KUKU_PROVIDER");
    std::env::remove_var("KUKU_ANTHROPIC_API_KEY");
    std::env::remove_var("KUKU_OPENAI_API_KEY");
    std::env::remove_var("KUKU_API_KEY");

    let err = query("test").session(sid).run().await.unwrap_err();
    assert!(matches!(err, Error::MissingProviderConfig(_)));

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelRequest { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ModelError { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::TurnEnd { .. })));

    restore_env("KUKU_PROVIDER", saved_provider);
    restore_env("KUKU_ANTHROPIC_API_KEY", saved_anthropic_key);
    restore_env("KUKU_OPENAI_API_KEY", saved_openai_key);
    restore_env("KUKU_API_KEY", saved_key);
}

#[tokio::test(flavor = "current_thread")]
async fn api_key_is_not_written_to_events() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let sid = "s_no_leak";

    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200).json_body(serde_json::json!({
            "id": "msg_2",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }));
    });

    query("test")
        .provider(Provider::Anthropic)
        .model("m")
        .api_key("secret-123")
        .base_url(server.base_url())
        .session(sid)
        .run()
        .await
        .unwrap();

    let events = EventStore::replay(env.events_path(sid)).unwrap();
    let raw = format!("{events:?}");
    assert!(!raw.contains("secret-123"));
}

fn restore_env(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}
