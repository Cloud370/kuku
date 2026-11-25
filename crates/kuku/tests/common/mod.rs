use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use kuku::session::session_events_path;

#[allow(
    dead_code,
    reason = "this shared fixture is used by a subset of integration binaries"
)]
pub fn shell_args(command: &str) -> Value {
    if cfg!(windows) {
        serde_json::json!({"program":"cmd.exe","args":["/D", "/S", "/C", command]})
    } else {
        serde_json::json!({"program":"sh","args":["-c", command]})
    }
}

fn captured_http_requests() -> &'static Mutex<BTreeMap<String, Vec<Value>>> {
    static REQUESTS: OnceLock<Mutex<BTreeMap<String, Vec<Value>>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Record model wire bodies for assertions against committed context views.
#[allow(
    dead_code,
    reason = "this shared fixture is used by a subset of integration binaries"
)]
pub fn capture_http_request(request: &httpmock::prelude::HttpMockRequest) -> bool {
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default())
        .expect("model request is JSON");
    let mut requests = captured_http_requests().lock().unwrap();
    let bodies = requests.entry(request.path.clone()).or_default();
    if !bodies.contains(&body) {
        bodies.push(body);
    }
    true
}

/// Take captured bodies for one test's unique model endpoint path.
#[allow(
    dead_code,
    reason = "this shared fixture is used by a subset of integration binaries"
)]
pub fn take_http_requests(path: &str) -> Vec<Value> {
    captured_http_requests()
        .lock()
        .unwrap()
        .remove(path)
        .unwrap_or_default()
}

// ---------- SSE response builders ----------

#[cfg(feature = "test_support")]
#[allow(
    unused_imports,
    reason = "this shared fixture is used by a subset of integration binaries"
)]
pub use kuku::test_support::anthropic_sse_response;

/// Wrap an OpenAI-style chat completion JSON into SSE streaming frames.
#[allow(
    dead_code,
    reason = "this shared fixture is used by a subset of integration binaries"
)]
pub fn openai_sse_response(completion: Value) -> String {
    let id = completion
        .get("id")
        .cloned()
        .unwrap_or(Value::String("chatcmpl-1".into()));
    let model = completion
        .get("model")
        .cloned()
        .unwrap_or(Value::String("test-model".into()));
    let usage = completion.get("usage").cloned();
    let choices = completion
        .get("choices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut sse = String::new();

    if let Some(choice) = choices.first() {
        let message = choice.get("message");
        let finish_reason = choice.get("finish_reason").and_then(Value::as_str);

        // Text content
        if let Some(text) = message
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
        {
            if !text.is_empty() {
                sse.push_str(&format!(
                    "data: {}\n\n",
                    serde_json::json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]})
                ));
            }
        }

        // Tool calls
        if let Some(tool_calls) = message
            .and_then(|m| m.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for (i, tc) in tool_calls.iter().enumerate() {
                let tc_id = tc.get("id").and_then(Value::as_str).unwrap_or("tc_1");
                let function = tc.get("function");
                let name = function
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let args = function
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                // Tool call start
                sse.push_str(&format!(
                    "data: {}\n\n",
                    serde_json::json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":i,"id":tc_id,"type":"function","function":{"name":name,"arguments":""}}]},"finish_reason":null}]})
                ));
                // Args
                if args != "{}" && !args.is_empty() {
                    sse.push_str(&format!(
                        "data: {}\n\n",
                        serde_json::json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":i,"function":{"arguments":args}}]},"finish_reason":null}]})
                    ));
                }
            }
        }

        // Finish reason
        if let Some(reason) = finish_reason {
            sse.push_str(&format!(
                "data: {}\n\n",
                serde_json::json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{},"finish_reason":reason}]})
            ));
        }
    }

    // Usage
    if let Some(u) = usage {
        sse.push_str(&format!(
            "data: {}\n\n",
            serde_json::json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[],"usage":u})
        ));
    }

    sse.push_str("data: [DONE]\n\n");
    sse
}

#[path = "../../src/test_process.rs"]
#[allow(
    dead_code,
    reason = "only tests that need process settings launch an isolated child"
)]
pub(crate) mod process;

#[allow(
    dead_code,
    reason = "only environment-dependent tests use this fixture"
)]
pub struct TestEnv {
    pub home: PathBuf,
    pub workspace: PathBuf,
}

impl TestEnv {
    #[allow(
        dead_code,
        reason = "only environment-dependent tests use this fixture"
    )]
    pub fn enter(module: &str, name: &str) -> Option<Self> {
        let in_child = process::isolate_test(module, name, |command| {
            let home = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            command
                .env_remove("KUKU_PROVIDER_TRACE")
                .env("KUKU_HOME", home.path())
                .current_dir(workspace.path());
            (home, workspace)
        });
        in_child.then(|| Self {
            home: std::env::var_os("KUKU_HOME")
                .expect("fixture sets KUKU_HOME")
                .into(),
            workspace: std::env::current_dir().expect("fixture sets the working directory"),
        })
    }

    #[allow(
        dead_code,
        reason = "integration targets use different fixture helpers"
    )]
    pub fn workspace_path(&self) -> &Path {
        &self.workspace
    }

    #[allow(
        dead_code,
        reason = "integration targets use different fixture helpers"
    )]
    pub fn events_path(&self, session_id: &str) -> PathBuf {
        let workspace = std::fs::canonicalize(&self.workspace).unwrap();
        session_events_path(&self.home, &workspace, session_id).unwrap()
    }
}

// ---------- Test config helpers ----------

/// Build a fixture with two named connections and an initial model on Anthropic.
#[allow(
    dead_code,
    reason = "this shared fixture is used by a subset of integration binaries"
)]
pub fn test_config() -> kuku::config::Config {
    use kuku::config::{
        ApiKey, Config, DiscoveryConfig, HandoffConfig, ModelConfig, ModelRef, ProviderConfig,
        ThinkLevel,
    };
    use std::collections::BTreeMap;

    let default_model = ModelRef::new("anthropic", "claude-sonnet-4-6");
    let models = BTreeMap::from([(
        default_model.clone(),
        ModelConfig {
            request: None,
            reasoning_effort: None,
            service_tier: None,
            think: ThinkLevel::Medium,
            context_window: 200_000,
            max_output_tokens: Some(48_000),
            purpose: String::new(),
            output: Default::default(),
        },
    )]);

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
            base_url: "https://api.anthropic.com".to_string(),
            api_key: ApiKey::Plaintext("unused".to_string()),
        },
    );
    providers.insert(
        "openai".to_string(),
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
            format: kuku::config::ProviderFormat::OpenAiChat,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: ApiKey::Plaintext("unused".to_string()),
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
        update: kuku::config::UpdateConfig::default(),
    }
}

#[allow(
    dead_code,
    reason = "this shared fixture is used by a subset of integration binaries"
)]
pub fn set_test_model<'a>(
    config: &'a mut kuku::config::Config,
    connection: &str,
    model: &str,
) -> &'a mut kuku::config::ModelConfig {
    let previous = config.models.remove(&config.default_model).unwrap();
    config.default_model = kuku::config::ModelRef::new(connection, model);
    config
        .models
        .entry(config.default_model.clone())
        .or_insert(previous)
}
