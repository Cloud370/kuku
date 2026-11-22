use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::execution::{CommandExecutionFacts, CommandTermination};
use crate::permission::{ToolAuthorizer, TrustedLocal};
use crate::tool::ToolResultEnvelope;

/// Durable status of a command or workspace script transferred to a Session Job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// The original execution is still in progress.
    Running,
    /// The execution reported success.
    Completed,
    /// The execution reported a terminal failure or could not proceed.
    Failed,
    /// The execution acknowledged cancellation and cleanup.
    Cancelled,
    /// A terminal outcome or cleanup could not be confirmed.
    Interrupted,
}

impl JobStatus {
    pub(super) fn for_result(outcome: &ToolResultEnvelope) -> Self {
        if outcome.faults.cleanup_incomplete {
            return Self::Interrupted;
        }
        match outcome.status.as_str() {
            "ok" => {
                let failed_command = outcome
                    .structured
                    .as_ref()
                    .and_then(|value| CommandExecutionFacts::deserialize(value).ok())
                    .is_some_and(|command| {
                        command.termination == CommandTermination::Exited
                            && (command.signal.is_some()
                                || command.exit_code.is_some_and(|code| code != 0))
                    });
                if failed_command {
                    Self::Failed
                } else {
                    Self::Completed
                }
            }
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }
}

/// Observed terminal facts for the original execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobResult {
    /// Final tool result status.
    pub status: String,
    /// Short result summary.
    pub summary: String,
    /// Typed tool facts, including original retained output references where available.
    pub structured: Option<serde_json::Value>,
    /// Whether the tool omitted part of its model-visible result.
    pub truncated: bool,
    /// Whether capture failed to publish its expected retained output.
    pub output_persistence_failed: bool,
    /// Whether cleanup of owned resources could not be confirmed.
    pub cleanup_incomplete: bool,
}

impl JobResult {
    pub(super) fn output_id(&self) -> Option<&str> {
        self.structured
            .as_ref()
            .and_then(|value| value["output_ref"]["id"].as_str())
    }
}

/// Session-scoped handle and status for an execution's background lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSnapshot {
    /// Stable Job identifier.
    pub id: String,
    /// Address of the Agent that owns the Job.
    pub agent: String,
    /// Turn where the original tool call was dispatched.
    pub turn: u64,
    /// Identity of the original tool call.
    pub tool_call_id: String,
    /// Tool that owns the execution.
    pub tool: String,
    /// Current durable Job status.
    pub status: JobStatus,
    /// Display title captured when the execution moved into the background.
    #[serde(default)]
    pub title: String,
    /// Monotonic time since dispatch when queried live, or measured time through settlement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// Short description of the running Job or its terminal result.
    pub summary: String,
    /// Retained output identity for a native command, available after capture is finalized.
    pub output_id: Option<String>,
    /// Durable cursor of the original tool result that registered the Job.
    pub started_at: u64,
    /// Durable cursor of the terminal Job event.
    pub finished_at: Option<u64>,
    /// Terminal facts loaded by a targeted lookup or cancellation.
    pub result: Option<JobResult>,
}

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
    /// Optional host-published route updates for later preparation boundaries.
    pub model_config_updates: Option<Arc<arc_swap::ArcSwap<Config>>>,
    /// Optional external prompt asset directory.
    pub prompts_dir: Option<PathBuf>,
    /// Session resource limits.
    pub limits: RuntimeLimits,
    /// Capacity shared by disposable script workers on this host.
    pub workspace_host: crate::WorkspaceExecutionHost,
    /// Host authority consulted before every tool dispatch.
    pub authorizer: Arc<dyn ToolAuthorizer>,
    /// Optional host model transport; preparation and dispatch limits remain owned by the SDK.
    pub model_adapter: Option<Arc<dyn crate::ModelAdapter>>,
    /// Provider diagnostics policy. Auto follows the build and process environment.
    pub provider_trace: crate::ProviderTraceMode,
    /// Additional finite tool definitions captured when this runtime opens.
    pub tools: Vec<Arc<dyn crate::ToolAdapter>>,
}

impl SessionOptions {
    pub(crate) fn model_config(&self) -> std::borrow::Cow<'_, Config> {
        let Some(updates) = &self.model_config_updates else {
            return std::borrow::Cow::Borrowed(&self.config);
        };
        let mut current = (**updates.load()).clone();
        if current.tier(&self.config.default_tier).is_some() {
            current.default_tier.clone_from(&self.config.default_tier);
        }
        std::borrow::Cow::Owned(current)
    }

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
            model_config_updates: None,
            prompts_dir: None,
            limits: RuntimeLimits::default(),
            workspace_host: crate::WorkspaceExecutionHost::default(),
            authorizer: Arc::new(TrustedLocal),
            model_adapter: None,
            provider_trace: crate::ProviderTraceMode::Auto,
            tools: Vec::new(),
        }
    }
}

/// Hard bounds for input, model execution, tools, and observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLimits {
    /// Limits applied to each embedded workspace script.
    #[serde(default)]
    pub workspace_script: crate::WorkspaceScriptLimits,
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
    /// Optional total model attempts shared by all agents and turns until this runtime closes.
    pub max_requests_per_open: Option<u64>,
    /// Whether to retry transient provider failures and regenerate malformed tool responses.
    #[serde(default = "default_provider_retry")]
    pub retry_transient_provider_errors: bool,
    /// Optional host output-token ceiling applied to every agent's resolved request.
    pub max_output_tokens_per_request: Option<u32>,
    /// Maximum simultaneous tools within one agent's step.
    pub max_tools_per_agent: usize,
    /// Foreground wait in milliseconds before a top-level command or script becomes a Job.
    #[serde(default = "default_job_foreground_wait_ms")]
    pub job_foreground_wait_ms: u64,
    /// Optional process deadline in seconds for direct commands and commands run by scripts.
    pub max_command_seconds: Option<std::num::NonZeroU32>,
    /// Maximum retained bytes for one command or host-tool output.
    pub max_output_bytes: u64,
    /// Maximum total physical bytes retained in this session, including capture framing.
    #[serde(default = "default_retained_output_bytes")]
    pub max_retained_output_bytes: u64,
    /// Minimum age, in seconds, before unreferenced ordinary output can be reclaimed.
    #[serde(default = "default_output_retention_secs")]
    pub output_retention_secs: u64,
    /// Maximum number of active named bindings across the session's agents.
    #[serde(default = "default_named_values")]
    pub max_named_values: usize,
    /// Maximum bytes in a direct file read or search page, or first retained-output preview.
    pub max_preview_bytes: usize,
    /// Maximum transient observations retained for slow subscribers.
    pub observation_capacity: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            workspace_script: crate::WorkspaceScriptLimits::default(),
            max_agents: 64,
            max_pending_inputs: 128,
            max_input_bytes: 65_536,
            max_request_bytes: 512 * 1024,
            max_model_requests: 4,
            max_requests_per_open: None,
            retry_transient_provider_errors: default_provider_retry(),
            max_output_tokens_per_request: None,
            max_tools_per_agent: 8,
            job_foreground_wait_ms: default_job_foreground_wait_ms(),
            max_command_seconds: None,
            max_output_bytes: 8 * 1024 * 1024,
            max_retained_output_bytes: default_retained_output_bytes(),
            output_retention_secs: default_output_retention_secs(),
            max_named_values: default_named_values(),
            max_preview_bytes: 8 * 1024,
            observation_capacity: 128,
        }
    }
}

fn default_provider_retry() -> bool {
    true
}

fn default_job_foreground_wait_ms() -> u64 {
    10_000
}

fn default_retained_output_bytes() -> u64 {
    128 * 1024 * 1024
}

fn default_output_retention_secs() -> u64 {
    24 * 60 * 60
}

fn default_named_values() -> usize {
    64
}

/// Agent-local behavior and explicit model selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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
    /// Media output overriding the route default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::config::RequestedOutput>,
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
    /// Optional final tool selection overriding preset defaults.
    pub tools: Option<Vec<String>>,
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

impl InputSource {
    const SESSION_JOB: &str = "session_job";

    pub(super) fn session_job() -> Self {
        Self::External(Self::SESSION_JOB.into())
    }

    pub(crate) fn is_job_delivery(&self) -> bool {
        matches!(self, Self::External(source) if source == Self::SESSION_JOB)
    }

    pub(crate) fn message_text(&self, text: String) -> String {
        if self.is_job_delivery() {
            let text = text.replace('&', "&amp;").replace('<', "&lt;");
            format!("<kuku_job_result>\n{text}\n</kuku_job_result>")
        } else {
            text
        }
    }

    pub(crate) fn agent_address(&self) -> Option<&str> {
        match self {
            Self::Agent(address) => Some(address),
            Self::Host(_) | Self::External(_) => None,
        }
    }
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

/// The earliest execution boundary at which a delivered input may be accepted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputTiming {
    /// Join the next model step in the current turn, or the next turn when idle.
    #[default]
    NextStep,
    /// Wait for the next turn, even when the current turn has more steps.
    NextTurn,
}

impl InputTiming {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
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
    /// Earliest safe boundary for acceptance.
    #[serde(default, skip_serializing_if = "InputTiming::is_default")]
    pub timing: InputTiming,
    /// Native skill requested for the model step that accepts this input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_skill: Option<String>,
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
            timing: InputTiming::NextStep,
            bootstrap_skill: None,
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

/// Durable state of a delivered input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputStatus {
    /// The input can still be withdrawn before acceptance.
    Queued,
    /// The input crossed an execution boundary.
    Accepted,
    /// The input was withdrawn before execution accepted it.
    Withdrawn,
}

/// A delivered input awaiting acceptance and its original receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInput {
    /// Durable delivery acknowledgement.
    pub receipt: Receipt,
    /// Original input and requested timing.
    pub input: Input,
}

/// The durable outcome of attempting to withdraw one delivered input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withdrawal {
    /// Durable acknowledgement of the withdrawal command.
    pub receipt: Receipt,
    /// Actual state of the referenced input at the time of this command.
    pub status: InputStatus,
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

/// Cumulative token usage and the latest request measurement for one agent.
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
    /// USD amounts and evidence counts for this agent's requests.
    #[serde(default)]
    pub costs: CostTotals,
    /// Requests that reported their total input tokens.
    #[serde(default)]
    pub input_reported_requests: u64,
    /// Requests that reported their total output tokens.
    #[serde(default)]
    pub output_reported_requests: u64,
    /// Total input tokens from requests reporting both input and cache reads.
    #[serde(default)]
    pub cache_measured_input_tokens: u64,
    /// Cache-read input tokens from those same requests.
    #[serde(default)]
    pub cache_measured_read_tokens: u64,
    /// Output tokens summed from requests with a known output count and positive duration.
    #[serde(default)]
    pub measured_output_tokens: u64,
    /// Total microseconds spent on the requests counted in measured output tokens.
    #[serde(default)]
    pub measured_elapsed_us: u64,
    /// Provider measurement for the most recently prepared request, once available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_request: Option<ModelUsageSample>,
}

/// Request amounts rounded to billionths of a US dollar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostTotals {
    pub reported_nanodollars: u64,
    pub estimated_nanodollars: u64,
    pub reported_requests: u64,
    pub estimated_requests: u64,
}

impl UsageTotals {
    /// Requests with unavailable monetary amounts, including unsettled attempts.
    pub fn unknown_cost_requests(&self) -> u64 {
        self.model_requests
            .saturating_sub(self.costs.reported_requests)
            .saturating_sub(self.costs.estimated_requests)
    }

    /// Return whether every counted request reported its input tokens.
    pub fn input_tokens_complete(&self) -> bool {
        self.complete(self.input_tokens, self.input_reported_requests)
    }

    /// Return whether every counted request reported its output tokens.
    pub fn output_tokens_complete(&self) -> bool {
        self.complete(self.output_tokens, self.output_reported_requests)
    }

    /// Return the latest reported input as a percentage of that request's context window.
    pub fn context_percent(&self) -> Option<f64> {
        let latest = self.latest_request.as_ref()?;
        let window = latest.context_window_tokens.filter(|window| *window > 0)?;
        Some(latest.input_tokens? as f64 / f64::from(window) * 100.0)
    }

    /// Return the token-weighted cache-read percentage over requests with paired counts.
    pub fn cache_hit_percent(&self) -> Option<f64> {
        let input = self.cache_measured_input_tokens;
        let read = self.cache_measured_read_tokens;
        (input > 0 && read <= input).then(|| read as f64 / input as f64 * 100.0)
    }

    /// Return the latest provider output tokens per second from dispatch to stream completion.
    pub fn output_tokens_per_second(&self) -> Option<f64> {
        let latest = self.latest_request.as_ref()?;
        output_rate(latest.output_tokens?, latest.elapsed_us?)
    }

    /// Return total measured output tokens divided by total measured request duration.
    pub fn average_output_tokens_per_second(&self) -> Option<f64> {
        output_rate(self.measured_output_tokens, self.measured_elapsed_us)
    }

    fn complete(&self, tokens: Option<u64>, reported: u64) -> bool {
        self.model_requests > 0
            && tokens.is_some()
            && (reported == self.model_requests || self.unreported_requests == 0)
    }
}

fn output_rate(output_tokens: u64, elapsed_us: u64) -> Option<f64> {
    (elapsed_us > 0).then(|| output_tokens as f64 * 1_000_000.0 / elapsed_us as f64)
}

/// Provider usage and timing bound to one prepared model request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsageSample {
    /// Total input tokens in this request, including cached input.
    pub input_tokens: Option<u64>,
    /// Total output tokens reported by the provider for this request.
    pub output_tokens: Option<u64>,
    /// Configured context window bound to this request at preparation.
    pub context_window_tokens: Option<u32>,
    /// Microseconds from dispatch of the serving attempt to its terminal stream marker.
    pub elapsed_us: Option<u64>,
}

/// Statistics for one completed or interrupted Agent Turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnSummary {
    /// Wall-clock milliseconds from first admission to terminal settlement, if recorded.
    pub elapsed_ms: Option<u64>,
    /// Total input tokens when all attempts reported them.
    pub input_tokens: Option<u64>,
    /// Total output tokens when all attempts reported them.
    pub output_tokens: Option<u64>,
    /// Token-weighted cache-read percentage over attempts with paired counts.
    pub cache_hit_percent: Option<f64>,
    /// The Turn reused a comparable prefix and had a low cache-read rate.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub low_cache_hit: bool,
    /// Number of committed tool calls, including calls waiting for authorization.
    pub tool_calls: u64,
}

/// Two or more consecutive comparable Turns with low measured cache reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheWarning {
    /// The first Turn that raised this warning episode.
    pub since_turn: u64,
    /// Whole percent shown in the footer for the latest comparable Turn.
    pub latest_percent: u8,
    /// Cache reads and input tokens from the last two comparable Turns.
    pub read_tokens: u64,
    pub input_tokens: u64,
}

/// Queryable state of an agent without consuming its output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Stable session-local address.
    pub address: String,
    /// The agent's current local configuration.
    pub config: AgentConfig,
    /// Configuration bound to the current execution, until the next safe boundary applies changes.
    #[serde(default)]
    pub active_config: Option<AgentConfig>,
    /// Current execution state.
    pub status: AgentStatus,
    /// Number of delivered inputs awaiting acceptance.
    pub pending_inputs: usize,
    /// Number of background Jobs still running for this Agent.
    #[serde(default)]
    pub running_jobs: usize,
    /// Bounded excerpt of the most recent admitted task input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_task: Option<String>,
    /// Bounded excerpt of the most recent task input still queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_task: Option<String>,
    /// Latest turn number, when any work was accepted.
    pub turn: Option<u64>,
    /// Unix time in milliseconds at the latest Turn's first admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_started_at_ms: Option<u64>,
    /// Final wall time for the latest Turn, present only after terminal settlement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_elapsed_ms: Option<u64>,
    /// Measured low cache reads on the current route, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_warning: Option<CacheWarning>,
    /// Durable identity of the observed execution, including a pending wait or cleanup.
    #[serde(default)]
    pub execution_id: Option<u64>,
    /// Last durable assistant response, independent of task success.
    pub last_output: Option<String>,
    /// Last execution failure, if any.
    pub last_error: Option<String>,
    /// Pending authorization requiring a matching reply.
    pub permission: Option<crate::query::PermissionRequest>,
    /// Pending question requiring a matching host reply.
    pub question: Option<QuestionRequest>,
    /// Cumulative reported usage.
    #[serde(default)]
    pub usage: UsageTotals,
}

/// Consistent session state and journal cursor for a selected agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentView {
    /// Selected agent's current state.
    pub snapshot: AgentSnapshot,
    /// Retained agent states in stable address order.
    pub agents: Vec<AgentSnapshot>,
    /// Latest committed event covered by this view.
    pub cursor: u64,
    /// Selected agent's retractable deliveries in receipt order.
    pub pending_inputs: Vec<PendingInput>,
}

/// Monetary usage across all retained Agents at one Session snapshot cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCostSummary {
    pub reported_nanodollars: u64,
    pub estimated_nanodollars: u64,
    pub unknown_requests: u64,
}

impl AgentView {
    pub fn session_costs(&self) -> SessionCostSummary {
        self.agents
            .iter()
            .fold(SessionCostSummary::default(), |mut total, agent| {
                let usage = &agent.usage;
                total.reported_nanodollars = total
                    .reported_nanodollars
                    .saturating_add(usage.costs.reported_nanodollars);
                total.estimated_nanodollars = total
                    .estimated_nanodollars
                    .saturating_add(usage.costs.estimated_nanodollars);
                total.unknown_requests = total
                    .unknown_requests
                    .saturating_add(usage.unknown_cost_requests());
                total
            })
    }
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
