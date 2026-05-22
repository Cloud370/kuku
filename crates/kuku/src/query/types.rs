use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::{ProviderFailure, ProviderToolCall, ResolvedProvider};
use crate::tool::ToolDefinition;

#[derive(Debug, Clone)]
/// Builder for configuring and executing a model query.
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

#[derive(Debug, Clone, PartialEq, Eq)]
/// Final output from a completed query run.
pub struct RunOutput {
    pub session_id: String,
    pub text: String,
    pub usage: Option<crate::provider::types::ProviderUsage>,
    pub turn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A pending permission request for a tool call.
pub struct PermissionRequest {
    pub id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub risk: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The host's response to a permission request.
pub enum PermissionChoice {
    Once,
    Session,
    Project,
    Deny,
}

/// Identifies the kind of a sub-execution phase (agent or long-running tool).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SubexecKind {
    Agent {
        child_session_id: String,
    },
    Tool {
        tool_name: String,
        tool_call_id: String,
    },
}

/// Events produced during a sub-execution phase.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SubexecEvent {
    TextDelta { text: String },
    ThinkingDelta { text: String },
    ToolCall { tool_call_id: String, tool: String, summary: String },
    ToolResult { tool_call_id: String, name: String, status: String, summary: String },
    Stdout(String),
    Stderr(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Permission mode for child sessions.
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
    ToolCall {
        tool_call_id: String,
        tool: String,
        summary: String,
    },
    ToolResult {
        tool_call_id: String,
        name: String,
        status: String,
        summary: String,
        structured: Option<serde_json::Value>,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    Done {
        output: RunOutput,
        usage: Option<crate::provider::types::ProviderUsage>,
        turn: u64,
    },
    TurnStart {
        turn: u64,
    },
    Error {
        code: String,
        message: String,
    },
    ModelRequest {
        model: String,
        provider: String,
    },
    /// A sub-execution phase has started (agent tool or long-running command).
    SubexecStart {
        stage_id: String,
        kind: SubexecKind,
        label: String,
    },
    /// Real-time output from a sub-execution phase.
    SubexecOutput {
        stage_id: String,
        event: SubexecEvent,
    },
    /// A sub-execution phase has completed.
    SubexecEnd {
        stage_id: String,
        status: String,
        summary: String,
        result: Option<serde_json::Value>,
    },
}

#[derive(Debug)]
/// An active query execution that yields UI events via `next()`.
pub struct Run {
    pub(super) session_id: String,
    pub(super) state: RunState,
    pub(crate) cancel_token: Arc<tokio::sync::Notify>,
    pub(crate) lock_path: PathBuf,
}

#[derive(Debug)]
pub(super) enum RunState {
    Pending(Box<PendingRun>),
    Streaming(Box<StreamingChunkState>),
    WaitingForPermission(Box<PendingPermission>),
    BatchEvents(Box<PendingRun>, VecDeque<UiEvent>),
    InSubexec {
        pending: Box<PendingRun>,
        stage_id: String,
        kind: SubexecKind,
        child_run: Box<Run>,
        label: String,
        tool_call_id: String,
        agent_name: String,
    },
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
    pub(super) cumulative_input_tokens: u64,
    pub(super) cumulative_output_tokens: u64,
    pub(super) cumulative_cache_read_input_tokens: u64,
    pub(super) cumulative_cache_creation_input_tokens: u64,
    pub(super) resolved: Option<ResolvedRuntime>,
    pub(super) queued_tool_calls: VecDeque<QueuedToolCall>,
    pub(super) saved_tool_call: Option<QueuedToolCall>,
    pub(super) config: Arc<Config>,
    pub(super) prompts_dir: Option<PathBuf>,
    pub(super) subagent_registry: Option<crate::subagent::registry::SubagentRegistry>,
    pub(super) skill_registry: Option<crate::skill::registry::SkillRegistry>,
    pub(super) skill_content_hash: Option<String>,
    pub(super) skill_body: Option<String>,
    pub(super) child_session_count: u32,
    pub(super) tool_registry_override: Option<Vec<crate::tool::ToolDefinition>>,
    pub(super) cancel_token: Arc<tokio::sync::Notify>,
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
    pub(super) queued_tool_call: QueuedToolCall,
    pub(super) request: PermissionRequest,
}

pub(super) enum PendingStep {
    Pending(Box<PendingRun>),
    NeedPermission(Box<PendingPermission>),
    Streaming(Box<StreamingChunkState>),
    ToolResultReady {
        pending: Box<PendingRun>,
        ui_event: UiEvent,
    },
    BatchReady {
        pending: Box<PendingRun>,
        ui_events: Vec<UiEvent>,
    },
    InSubexec {
        pending: Box<PendingRun>,
        stage_id: String,
        kind: SubexecKind,
        child_run: Box<Run>,
        label: String,
        tool_call_id: String,
        agent_name: String,
    },
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
    pub(super) stream: Pin<
        Box<dyn Stream<Item = std::result::Result<ProviderChunk, ProviderFailure>> + Send + Sync>,
    >,
    pub(super) accumulated_text: String,
    pub(super) accumulated_thinking: String,
    pub(super) stop_reason: Option<String>,
    pub(super) tool_calls: Vec<ProviderToolCall>,
    pub(super) tool_arg_buffers: Vec<(u64, String)>,
    pub(super) provider_request_id: Option<String>,
    pub(super) usage: Option<crate::provider::types::ProviderUsage>,
    pub(super) lead_events: Vec<UiEvent>,
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
