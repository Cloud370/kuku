use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::permission::{ToolAuthorizer, TrustedLocal};

/// Configuration and resource ownership for opening a persistent session.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// Directory containing the files the agents work on.
    pub workspace: PathBuf,
    /// Root directory containing session state and retained output.
    pub data_dir: PathBuf,
    /// Stable session identity, generated when omitted.
    pub session_id: Option<String>,
    /// Host configuration containing named connections and model tiers.
    pub config: Config,
    /// Optional external prompt asset directory.
    pub prompts_dir: Option<PathBuf>,
    /// Session resource limits.
    pub limits: RuntimeLimits,
    /// Host authority consulted before every tool dispatch.
    pub authorizer: Arc<dyn ToolAuthorizer>,
}

impl SessionOptions {
    /// Configure a session without changing process directories or environment.
    pub fn new(
        workspace: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
        config: Config,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            data_dir: data_dir.into(),
            session_id: None,
            config,
            prompts_dir: None,
            limits: RuntimeLimits::default(),
            authorizer: Arc::new(TrustedLocal),
        }
    }
}

/// Hard bounds for input, model execution, tools, and observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLimits {
    /// Maximum agents retained by this session.
    pub max_agents: usize,
    /// Maximum delivered inputs waiting for one agent.
    pub max_pending_inputs: usize,
    /// Maximum UTF-8 bytes in one input.
    pub max_input_bytes: usize,
    /// Maximum serialized bytes in one prepared model request.
    pub max_request_bytes: usize,
    /// Maximum simultaneous model streams across the session.
    pub max_model_requests: usize,
    /// Maximum logical model requests in one turn, including recovery requests.
    pub max_steps_per_turn: u64,
    /// Maximum simultaneous tools within one agent's step.
    pub max_tools_per_agent: usize,
    /// Maximum retained bytes for one command output.
    pub max_output_bytes: u64,
    /// Maximum bytes in a command's first model-visible preview.
    pub max_preview_bytes: usize,
    /// Maximum transient observations retained for slow subscribers.
    pub observation_capacity: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_agents: 64,
            max_pending_inputs: 128,
            max_input_bytes: 65_536,
            max_request_bytes: 512 * 1024,
            max_model_requests: 4,
            max_steps_per_turn: 20,
            max_tools_per_agent: 8,
            max_output_bytes: 8 * 1024 * 1024,
            max_preview_bytes: 16 * 1024,
            observation_capacity: 128,
        }
    }
}

/// Agent-local behavior and explicit model selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Optional contact card supplying behavior and initial defaults.
    pub preset: Option<String>,
    /// Named connection, independent of its wire protocol.
    pub connection: Option<String>,
    /// Model tier used for reusable routing defaults.
    pub tier: Option<String>,
    /// Explicit model identifier overriding tier selection.
    pub model: Option<String>,
    /// Explicit provider-supported reasoning setting.
    pub reasoning_effort: Option<String>,
    /// Explicit provider-supported service setting.
    pub service_tier: Option<String>,
    /// Maximum tokens returned by one model request.
    pub max_output_tokens: Option<u32>,
    /// Behavior instructions for this agent only.
    pub instructions: Option<String>,
    /// Optional working directory overriding the session default.
    pub workspace: Option<PathBuf>,
    /// Disable native skill discovery and loading for this agent.
    pub no_skills: bool,
    /// Disable the default session agent collaboration tools.
    pub no_agents: bool,
    /// Optional native skill loaded at this agent's first accepted turn.
    pub bootstrap_skill: Option<String>,
}

/// Trusted origin of an input or tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "identity", rename_all = "snake_case")]
pub enum InputSource {
    /// Input submitted by the embedding host or local user.
    Host(String),
    /// Input sent through an agent's authorized tool boundary.
    Agent(String),
    /// Input submitted by a host's external event adapter.
    External(String),
}

/// Whether delivery also requests advancement of an idle recipient.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    /// Persist the input and wake an idle recipient.
    #[default]
    Wake,
    /// Persist the input for explicit advancement by the host.
    Enqueue,
}

/// Idempotent delivery to one stable address within a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    /// Caller-selected command identity, unique within this session.
    pub command_id: String,
    /// Trusted origin supplied by the caller boundary.
    pub source: InputSource,
    /// Flat, stable agent address.
    pub target: String,
    /// Material selected for this recipient's independent context.
    pub text: String,
    /// Whether an idle recipient should advance.
    pub activation: Activation,
    /// Optional external work or message association.
    pub correlation: Option<String>,
}

impl Input {
    /// Create an input from a trusted local host.
    pub fn new(
        command_id: impl Into<String>,
        target: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            command_id: command_id.into(),
            source: InputSource::Host("local".into()),
            target: target.into(),
            text: text.into(),
            activation: Activation::Wake,
            correlation: None,
        }
    }
}

/// Durable acknowledgement of a command, distinct from execution completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Original caller command identity.
    pub command_id: String,
    /// Event cursor at which the command became durable.
    pub event_id: u64,
    /// Target agent address.
    pub agent: String,
}

/// Current execution state reconstructed from committed session facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// No execution is advancing.
    Idle,
    /// The agent owns one active execution chain.
    Running,
    /// A matching host reply or authorization is required.
    Waiting,
    /// Explicitly stopped until an authorized advance operation.
    Stopped,
    /// An interrupted execution needs an explicit host decision.
    Interrupted,
}

/// Cumulative token usage reported by completed model requests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    /// Sum of reported total input tokens, including cached input.
    pub input_tokens: Option<u64>,
    /// Output tokens when reported.
    pub output_tokens: Option<u64>,
    /// Input tokens read from provider caches when reported.
    pub cache_read_input_tokens: Option<u64>,
    /// Input tokens written to provider caches when reported.
    pub cache_creation_input_tokens: Option<u64>,
    /// Prepared model request attempts, including missing responses.
    pub model_requests: u64,
    /// Responses missing either input or output usage, so reported totals may be partial.
    pub unreported_requests: u64,
}

/// Queryable state of an agent without consuming its output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Stable session-local address.
    pub address: String,
    /// The agent's current local configuration.
    pub config: AgentConfig,
    /// Current execution state.
    pub status: AgentStatus,
    /// Number of delivered inputs awaiting acceptance.
    pub pending_inputs: usize,
    /// Latest turn number, when any work was accepted.
    pub turn: Option<u64>,
    /// Last durable assistant response, independent of task success.
    pub last_output: Option<String>,
    /// Last execution failure, if any.
    pub last_error: Option<String>,
    /// Pending authorization requiring a matching reply.
    pub permission: Option<crate::query::PermissionRequest>,
    /// Pending question requiring a matching host reply.
    pub question: Option<QuestionRequest>,
    /// Cumulative reported usage.
    pub usage: UsageTotals,
}

/// Durable request for selected information at an agent's safe waiting boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequest {
    /// Exact identity required by a later reply.
    pub id: String,
    /// Recipient agent address.
    pub agent: String,
    /// Turn that requested the information.
    pub turn: u64,
    /// Original tool call returning the waiting handle.
    pub tool_call_id: String,
    /// Question presented to the host.
    pub text: String,
}
