use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_core::Stream;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::{ProviderFailure, ProviderToolCall, ResolvedProvider};
use crate::tool::ToolDefinition;

/// Builder for configuring and executing a model query.
#[derive(Debug, Clone)]
pub struct Query {
    pub(super) prompt: String,
    pub(super) session_id: Option<String>,
    pub(super) provider: Option<crate::provider::Provider>,
    pub(super) model: Option<String>,
    pub(super) tier: Option<String>,
    pub(super) config_path: Option<PathBuf>,
    pub(super) config_obj: Option<Config>,
    pub(super) base_url: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) temperature: Option<f32>,
    pub(super) workspace_path: Option<PathBuf>,
    pub(super) prompts_dir: Option<PathBuf>,
    pub(super) disable_agents: bool,
    pub(super) disable_skills: bool,
    pub(super) skill_body: Option<String>,
    pub(super) subagent_registry: Option<crate::subagent::registry::SubagentRegistry>,
    pub(crate) tool_registry_override: Option<Vec<crate::tool::ToolDefinition>>,
}

/// Final output from a completed query run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub session_id: String,
    pub text: String,
    pub usage: Option<crate::provider::types::ProviderUsage>,
    pub turn: u64,
    pub model_request_count: u64,
    pub thinking_duration_ms: u64,
    pub tool_summary: ToolSummary,
    pub(super) plugin_registry: Option<Arc<crate::plugin::PluginRegistry>>,
    pub(super) session_dir: std::path::PathBuf,
    pub(super) workspace: std::path::PathBuf,
}

/// A pending permission request for a tool call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub risk: String,
    pub summary: String,
}

/// The host's response to a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    Once,
    Session,
    Project,
    Deny,
}

/// Identifies the kind of tool execution.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Simple,
    Agent { child_session_id: String },
    Command { pid: Option<u32> },
}

/// Events produced during a tool execution phase.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolStart {
        id: String,
        tool: String,
        summary: String,
        kind: ToolKind,
    },
    ToolOutput {
        id: String,
        event: Box<ToolEvent>,
    },
    ToolEnd {
        id: String,
        status: String,
        summary: String,
    },
    Stdout {
        text: String,
    },
    Stderr {
        text: String,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Permission mode for child sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    AutoAllow,
    Interactive,
}

/// Host-facing runtime event stream.
///
/// This enum is non-exhaustive; hosts must keep a fallback arm when matching it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolStart {
        id: String,
        tool: String,
        summary: String,
        kind: ToolKind,
    },
    ToolOutput {
        id: String,
        event: ToolEvent,
    },
    ToolEnd {
        id: String,
        status: String,
        summary: String,
        model_content: Option<String>,
        result: Option<serde_json::Value>,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    Error {
        code: String,
        message: String,
    },
    ModelRequest {
        model: String,
        provider: String,
    },
    TurnStart {
        turn: u64,
    },
    Done {
        output: RunOutput,
        usage: Option<crate::provider::types::ProviderUsage>,
        turn: u64,
    },
    Cancelled {
        turn: u64,
    },
}

/// An active query execution that yields UI events via `next()`.
#[derive(Debug)]
pub struct Run {
    pub(super) session_id: String,
    pub(super) state: RunState,
    pub(crate) slots: std::collections::HashMap<String, ExecSlot>,
    pub(crate) slot_event_tx: tokio::sync::mpsc::Sender<(String, SlotEvent)>,
    pub(crate) slot_event_rx: tokio::sync::mpsc::Receiver<(String, SlotEvent)>,
    pub(crate) cancel_token: Arc<tokio::sync::Notify>,
    pub(crate) lock_path: PathBuf,
}

pub(crate) struct ExecSlot {
    pub(crate) tool_call_id: String,
    pub(crate) kind: ToolKind,
    pub(crate) ordered_with_simple_tools: bool,
    pub(crate) label: String,
    pub(crate) cancel: Arc<tokio::sync::Notify>,
    pub(crate) child_permissions:
        Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<PermissionChoice>>>>,
}

impl std::fmt::Debug for ExecSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecSlot")
            .field("tool_call_id", &self.tool_call_id)
            .field("kind", &self.kind)
            .field("ordered_with_simple_tools", &self.ordered_with_simple_tools)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) enum SlotEvent {
    Output(ToolEvent),
    Done {
        status: String,
        summary: String,
        model_content: String,
        result: Option<serde_json::Value>,
    },
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // Done variant is large but boxed variants dominate hot paths
pub(super) enum RunState {
    Pending(Box<PendingRun>),
    Streaming(Box<StreamingChunkState>),
    WaitingForPermission(Box<PendingPermission>),
    Cancelled {
        events_path: std::path::PathBuf,
        turn: u64,
    },
    Done(
        Option<(
            RunOutput,
            Option<crate::provider::types::ProviderUsage>,
            u64,
        )>,
    ),
}

#[derive(Debug, Default)]
pub(super) struct CumulativeUsage {
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) cache_creation_input_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ToolSummary {
    pub total_calls: u64,
    pub names: Vec<String>,
    pub denied: u64,
    pub errors: u64,
    pub rounds: u64,
}

#[derive(Debug)]
pub(super) struct PendingRun {
    pub(super) session_id: String,
    pub(super) query: Query,
    pub(super) events_path: PathBuf,
    pub(super) kuku_home: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) policy_path: PathBuf,
    pub(super) turn: u64,
    pub(super) request_num: u64,
    pub(super) cumulative: CumulativeUsage,
    pub(super) resolved: Option<ResolvedRuntime>,
    pub(super) queued_tool_calls: VecDeque<QueuedToolCall>,
    pub(super) config: Arc<Config>,
    pub(super) prompts_dir: Option<PathBuf>,
    pub(super) subagent_registry: Option<crate::subagent::registry::SubagentRegistry>,
    pub(super) skill_registry: Option<crate::skill::registry::SkillRegistry>,
    pub(super) skill_content_hash: Option<String>,
    pub(super) skill_body: Option<String>,
    pub(super) child_session_count: u32,
    pub(super) tool_registry_override: Option<Vec<crate::tool::ToolDefinition>>,
    pub(super) catalog: crate::prompt::PromptCatalog,
    pub(super) pending_events: std::collections::VecDeque<UiEvent>,
    pub(super) cancel_token: Arc<tokio::sync::Notify>,
    pub(super) handoff_triggered: bool,
    pub(super) handoff_keep_turns: usize,
    pub(super) plugin_registry: Option<Arc<crate::plugin::PluginRegistry>>,
    pub(super) hook_context: Vec<String>,
    pub(super) force_continue_count: u64,
    pub(super) model_request_count: u64,
    pub(super) tool_rounds: u64,
    pub(super) tool_calls: u64,
    pub(super) tool_names: Vec<String>,
    pub(super) tool_denied: u64,
    pub(super) tool_errors: u64,
    pub(super) thinking_duration_ms: u64,
}

impl PendingRun {
    pub(super) fn record_tool_call(&mut self, name: &str) {
        self.tool_calls += 1;
        if !self.tool_names.iter().any(|n| n == name) {
            self.tool_names.push(name.to_owned());
        }
    }

    pub(super) fn record_tool_denied(&mut self, name: &str) {
        self.record_tool_call(name);
        self.tool_denied += 1;
    }

    pub(super) fn record_tool_error(&mut self, name: &str) {
        self.record_tool_call(name);
        self.tool_errors += 1;
    }
}

#[derive(Debug)]
pub(super) struct ResolvedRuntime {
    pub(super) config: ResolvedProvider,
    pub(super) registry: Vec<ToolDefinition>,
    pub(super) registry_hash: String,
    pub(super) tool_names: Vec<String>,
    pub(super) tool_count: usize,
}

#[derive(Debug)]
pub(super) struct QueuedToolCall {
    pub(super) tool_call: ProviderToolCall,
    pub(super) display_summary: String,
}

#[derive(Debug)]
pub(super) struct PendingPermission {
    pub(super) pending: PendingRun,
    pub(super) request: PermissionRequest,
}

pub(super) enum PendingStep {
    /// One tool call processed. May carry a spawned slot and/or a UI event.
    /// When both are None, advance_pending has no more queued calls to process.
    Pending {
        pending: Box<PendingRun>,
        slot: Option<ExecSlot>,
        event: Option<UiEvent>,
    },
    NeedPermission(Box<PendingPermission>),
    Streaming(Box<StreamingChunkState>),
    #[allow(clippy::type_complexity)]
    Done(
        RunOutput,
        Option<crate::provider::types::ProviderUsage>,
        u64,
    ),
}

pub(super) struct StreamingChunkState {
    pub(super) pending: PendingRun,
    pub(super) request_id: String,
    pub(super) stream:
        Pin<Box<dyn Stream<Item = std::result::Result<ProviderChunk, ProviderFailure>> + Send>>,
    pub(super) accumulated_text: String,
    pub(super) accumulated_thinking: String,
    pub(super) stop_reason: Option<String>,
    pub(super) tool_calls: Vec<ProviderToolCall>,
    pub(super) tool_arg_buffers: Vec<(u64, String)>,
    pub(super) provider_request_id: Option<String>,
    pub(super) usage: Option<crate::provider::types::ProviderUsage>,
    pub(super) lead_events: Vec<UiEvent>,
    pub(super) handoff_detector: Option<HandoffDetector>,
    pub(super) thinking_start: Option<std::time::Instant>,
    pub(super) thinking_duration_ms: u64,
}

use super::handoff::HandoffDetector;

#[cfg(test)]
impl RunOutput {
    pub(crate) fn new(
        session_id: String,
        text: String,
        usage: Option<crate::provider::types::ProviderUsage>,
        turn: u64,
    ) -> Self {
        Self {
            session_id,
            text,
            usage,
            turn,
            model_request_count: 0,
            thinking_duration_ms: 0,
            tool_summary: ToolSummary::default(),
            plugin_registry: None,
            session_dir: std::path::PathBuf::new(),
            workspace: std::path::PathBuf::new(),
        }
    }
}

impl std::fmt::Debug for StreamingChunkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingChunkState")
            .field("request_id", &self.request_id)
            .field("accumulated_text", &self.accumulated_text)
            .field("stop_reason", &self.stop_reason)
            .field("tool_calls", &self.tool_calls)
            .finish_non_exhaustive()
    }
}

// ---------- Query builder ----------

impl Query {
    /// Create a new query builder for the given prompt.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
            provider: None,
            model: None,
            tier: None,
            config_path: None,
            config_obj: None,
            base_url: None,
            api_key: None,
            max_output_tokens: None,
            temperature: None,
            workspace_path: None,
            prompts_dir: None,
            disable_agents: false,
            disable_skills: false,
            skill_body: None,
            subagent_registry: None,
            tool_registry_override: None,
        }
    }

    /// Register a subagent registry for agent tool dispatch.
    pub fn subagents(mut self, registry: crate::subagent::registry::SubagentRegistry) -> Self {
        self.subagent_registry = Some(registry);
        self
    }

    /// Disable the agent tool (subagent delegation).
    pub fn no_agents(mut self) -> Self {
        self.disable_agents = true;
        self
    }

    /// Disable the use_skill tool.
    pub fn no_skills(mut self) -> Self {
        self.disable_skills = true;
        self
    }

    /// Attach a skill body to be injected as a separate block before user input.
    pub fn skill_body(mut self, body: String) -> Self {
        self.skill_body = Some(body);
        self
    }

    /// Set or resume a session by ID.
    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set the provider to use for this query.
    pub fn provider(mut self, provider: crate::provider::Provider) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set the model alias or provider:model name.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Override the provider base URL.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Set the API key directly, bypassing config resolution.
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Set the maximum output token budget.
    pub fn max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Select a capability tier (strong/balanced/light). Mutually exclusive with `.model()`.
    pub fn tier(mut self, tier: impl Into<String>) -> Self {
        self.tier = Some(tier.into());
        self
    }

    /// Load config from the given path.
    pub fn config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Use an in-memory config (bypasses file loading).
    pub fn config(mut self, cfg: Config) -> Self {
        self.config_obj = Some(cfg);
        self
    }

    /// Set the workspace directory (defaults to cwd).
    pub fn workspace(mut self, path: impl Into<PathBuf>) -> Self {
        self.workspace_path = Some(path.into());
        self
    }

    /// Directory containing prompt files to override embedded defaults.
    pub fn prompts_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.prompts_dir = Some(path.into());
        self
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.tier.is_some() && self.model.is_some() {
            return Err(Error::InvalidArgument(
                ".tier() and .model() are mutually exclusive".to_string(),
            ));
        }
        Ok(())
    }

    /// The prompt text.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// The session ID, if set.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&ToolKind::Simple).unwrap(),
            r#""simple""#
        );
        let cmd = ToolKind::Command { pid: None };
        assert_eq!(
            serde_json::to_string(&cmd).unwrap(),
            r#"{"command":{"pid":null}}"#
        );
        let agent = ToolKind::Agent {
            child_session_id: "child_abc_0".into(),
        };
        let agent_json = serde_json::to_string(&agent).unwrap();
        let back: ToolKind = serde_json::from_str(&agent_json).unwrap();
        assert_eq!(back, agent);
    }

    #[test]
    fn tool_event_recursive_serialization() {
        let inner = ToolEvent::ToolStart {
            id: "tc_2".into(),
            tool: "find_files".into(),
            summary: "search".into(),
            kind: ToolKind::Simple,
        };
        let outer = ToolEvent::ToolOutput {
            id: "tc_1".into(),
            event: Box::new(inner),
        };
        let json = serde_json::to_string(&outer).unwrap();
        assert!(json.contains("tc_1"));
        assert!(json.contains("tc_2"));
    }

    #[test]
    fn tool_event_stdout_stderr_struct_variants() {
        let out = ToolEvent::Stdout {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("stdout"));
        assert!(json.contains("hello"));
        let back: ToolEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, out);
    }

    #[test]
    fn tool_summary_default_is_all_zeros() {
        let s = ToolSummary::default();
        assert_eq!(s.total_calls, 0);
        assert!(s.names.is_empty());
        assert_eq!(s.denied, 0);
        assert_eq!(s.errors, 0);
        assert_eq!(s.rounds, 0);
    }

    #[test]
    fn run_output_new_has_zero_counters() {
        let output = RunOutput::new("sid".into(), "text".into(), None, 1);
        assert_eq!(output.model_request_count, 0);
        assert_eq!(output.thinking_duration_ms, 0);
        assert_eq!(output.tool_summary, ToolSummary::default());
    }

    #[test]
    fn tool_summary_serializes_flat() {
        let s = ToolSummary {
            total_calls: 3,
            names: vec!["read_file".into(), "bash".into()],
            denied: 0,
            errors: 1,
            rounds: 2,
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["total_calls"], 3);
        assert_eq!(json["names"], serde_json::json!(["read_file", "bash"]));
        assert_eq!(json["denied"], 0);
        assert_eq!(json["errors"], 1);
        assert_eq!(json["rounds"], 2);
    }
}
