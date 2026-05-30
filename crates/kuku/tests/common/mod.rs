use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde_json::Value;
use tempfile::TempDir;

use kuku::session::session_events_path;

// ---------- SSE response builders ----------

/// Wrap an Anthropic-style message JSON into SSE streaming frames.
#[allow(dead_code)] // used by provider_integration and query_runtime test binaries
pub fn anthropic_sse_response(msg: Value) -> String {
    let id = msg
        .get("id")
        .cloned()
        .unwrap_or(Value::String("msg_1".into()));
    let model = msg
        .get("model")
        .cloned()
        .unwrap_or(Value::String("test-model".into()));
    let stop_reason = msg
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    let usage = msg
        .get("usage")
        .cloned()
        .unwrap_or(serde_json::json!({"input_tokens": 0, "output_tokens": 0}));
    let content = msg
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut sse = String::new();

    // message_start
    sse.push_str(&format!(
        "event: message_start\ndata: {}\n\n",
        serde_json::json!({"type":"message_start","message":{"id":id,"model":model,"content":[],"usage":usage}})
    ));

    // content blocks
    for (i, block) in content.iter().enumerate() {
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("text");
        if btype == "text" {
            let text = block.get("text").and_then(Value::as_str).unwrap_or("");
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}})
            ));
            if !text.is_empty() {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":text}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        } else if btype == "tool_use" {
            let tc_id = block.get("id").and_then(Value::as_str).unwrap_or("tc_1");
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":tc_id,"name":name,"input":{}}})
            ));
            let args_str = serde_json::to_string(&input).unwrap_or_default();
            if !args_str.is_empty() && args_str != "{}" {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":args_str}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        }
    }

    // message_delta
    sse.push_str(&format!(
        "event: message_delta\ndata: {}\n\n",
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":stop_reason},"usage":{"output_tokens":usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0)}})
    ));

    // message_stop
    sse.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");

    sse
}

/// Wrap an OpenAI-style chat completion JSON into SSE streaming frames.
#[allow(dead_code)] // used by provider_integration test binary
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

// ---------- Env isolation ----------

pub fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[allow(dead_code)] // used by provider_integration test binary
pub fn restore_env(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

// ---------- TestEnv for integration tests ----------

#[allow(dead_code)] // used by provider_integration and query_runtime test binaries
const PROVIDER_ENV_KEYS: &[&str] = &[
    "KUKU_PROVIDER",
    "KUKU_ANTHROPIC_API_KEY",
    "KUKU_OPENAI_API_KEY",
    "KUKU_API_KEY",
];

#[allow(dead_code)] // used by provider_integration and query_runtime test binaries
pub struct TestEnv {
    pub _guard: MutexGuard<'static, ()>,
    pub home: TempDir,
    pub workspace: TempDir,
    previous_kuku_home: Option<OsString>,
    previous_cwd: PathBuf,
    previous_provider_env: Vec<(&'static str, Option<OsString>)>,
}

impl TestEnv {
    #[allow(dead_code)] // used by provider_integration and query_runtime test binaries
    pub fn new() -> Self {
        let guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_kuku_home = std::env::var_os("KUKU_HOME");
        let previous_cwd = std::env::current_dir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();

        let previous_provider_env: Vec<_> = PROVIDER_ENV_KEYS
            .iter()
            .map(|&key| (key, std::env::var_os(key)))
            .collect();
        for &key in PROVIDER_ENV_KEYS {
            std::env::remove_var(key);
        }

        std::env::set_var("KUKU_HOME", home.path());
        std::env::set_current_dir(workspace.path()).unwrap();

        Self {
            _guard: guard,
            home,
            workspace,
            previous_kuku_home,
            previous_cwd,
            previous_provider_env,
        }
    }

    #[allow(dead_code)] // used by provider_integration test binary
    pub fn workspace_path(&self) -> &Path {
        self.workspace.path()
    }

    #[allow(dead_code)] // used by provider_integration and query_runtime test binaries
    pub fn events_path(&self, session_id: &str) -> PathBuf {
        let workspace = std::fs::canonicalize(self.workspace.path()).unwrap();
        session_events_path(self.home.path(), &workspace, session_id).unwrap()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, value) in &self.previous_provider_env {
            restore_env(key, value.clone());
        }
        std::env::set_current_dir(&self.previous_cwd).unwrap();
        match &self.previous_kuku_home {
            Some(value) => std::env::set_var("KUKU_HOME", value),
            None => std::env::remove_var("KUKU_HOME"),
        }
    }
}

// ---------- Test config helpers ----------

/// Build a minimal Config with anthropic + openai providers and a balanced tier.
/// Builder overrides (.model(), .base_url(), .api_key()) take precedence at resolution time.
#[allow(dead_code)]
pub fn test_config() -> kuku::config::Config {
    use kuku::config::{
        ApiKey, Config, DiscoveryConfig, HandoffConfig, ProviderConfig, ThinkLevel, TierConfig,
    };
    use std::collections::BTreeMap;

    let mut tiers = BTreeMap::new();
    tiers.insert(
        "balanced".to_string(),
        TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            think: ThinkLevel::Medium,
            context_window: 200_000,
            max_output_tokens: 48_000,
            purpose: "balanced".to_string(),
        },
    );

    let mut providers = BTreeMap::new();
    providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            format: kuku::config::ProviderFormat::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            api_key: ApiKey::Plaintext("unused".to_string()),
        },
    );
    providers.insert(
        "openai".to_string(),
        ProviderConfig {
            format: kuku::config::ProviderFormat::OpenAiChat,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: ApiKey::Plaintext("unused".to_string()),
        },
    );

    Config {
        tiers,
        providers,
        default_tier: "balanced".to_string(),
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        plugin: kuku::config::PluginConfig::default(),
    }
}
