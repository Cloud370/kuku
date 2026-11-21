//! Host-provided model transports and tools using the Session execution boundary.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{ProviderFormat, RequestedOutput};

use crate::permission::ToolAuthorization;
pub use crate::provider::{
    discover_models, model_list_targets, openrouter_quote_url, openrouter_quotes,
    plan_model_trials, route_quote_for_display, setup_api_candidates, setup_endpoint_format,
    setup_model_list_targets, setup_requires_responses_for_tools, trial_model, DiscoveredModel,
    ModelListTarget, ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, ProbeRow,
    ReasoningCheck, ReasoningCheckResult, RequestCost, RouteQuote, SetupApiEvidence,
    SetupTextCheck, TrialResult, TrialSelection, TRIAL_OUTPUT_TOKENS,
};
use crate::{ProviderCapabilities, ProviderSettings, Result};

pub use crate::provider::chunk::ProviderChunk as ModelEvent;

/// An asynchronous host operation which retains its borrowed adapter until completion.
pub type AdapterFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Normalized model events consumed by the existing execution state machine.
pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent>> + Send>>;

/// Frozen, credential-free wire request selected by the SDK before transport dispatch.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Owning session identity.
    pub session_id: String,
    /// Acting agent's stable address.
    pub agent: String,
    /// Owning turn identity.
    pub turn: u64,
    /// Request identity scoped to the owning Agent and Turn.
    pub request_id: String,
    /// Independently configured connection name.
    pub connection: String,
    /// Selected wire protocol.
    pub protocol: ProviderFormat,
    /// Selected model identifier.
    pub model: String,
    /// Prepared request URL, without authentication headers.
    pub endpoint: String,
    /// Exact prepared request body, already checked against the request byte limit.
    pub body: Value,
    /// Requested and effective reasoning/service settings.
    pub settings: ProviderSettings,
    /// Stable provider correlation identifier, when supplied.
    pub correlation_id: Option<String>,
}

/// Replace the default HTTP transport while preserving preparation, limits and persistence.
pub trait ModelAdapter: std::fmt::Debug + Send + Sync {
    /// Start a normalized stream. Dropping this future or stream must release its resources.
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream>;
}

/// Declared side effects used for policy and workspace ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAccess {
    /// Reads without joining the Agent's ordered workspace group; may overlap other tools.
    Read,
    /// Mutates state and joins the Agent's ordered workspace action group.
    Write,
    /// Starts external actions and joins the Agent's ordered workspace action group.
    Execute,
}

/// Immutable tool definition captured when the Session opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique tool name; built-in tool names cannot be replaced.
    pub name: String,
    /// Model-visible description.
    pub description: String,
    /// JSON object describing accepted arguments.
    pub input_schema: Value,
    /// Declared side effects and ordering.
    pub access: ToolAccess,
    /// Maximum bytes in the first model-visible result, including its output reference.
    pub max_result_bytes: usize,
}

impl ToolSpec {
    /// Define a tool with a 16 KiB first result view.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        access: ToolAccess,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            access,
            max_result_bytes: 16 * 1024,
        }
    }

    pub(crate) fn definition(&self) -> crate::tool::ToolDefinition {
        crate::tool::ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            read_only: self.access == ToolAccess::Read,
            max_result_chars: self.max_result_bytes,
            risk: match self.access {
                ToolAccess::Read => "read",
                ToolAccess::Write => "edit",
                ToolAccess::Execute => "execute",
            }
            .into(),
        }
    }
}

/// Cooperative cancellation signal. A tool returns only after cleaning up its owned resources.
#[derive(Debug, Clone)]
pub struct ToolCancellation(pub(crate) tokio::sync::watch::Receiver<bool>);

impl ToolCancellation {
    /// Whether cancellation has already been requested.
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    /// Wait until cancellation is requested, including a request made before this call.
    pub async fn cancelled(&self) {
        let mut receiver = self.0.clone();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

/// Terminal outcome of a host tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Work completed successfully.
    Ok,
    /// Work failed.
    Error,
    /// Work was rejected without dispatching side effects.
    Blocked,
    /// Work was cancelled and cleanup finished.
    Cancelled,
}

impl ToolStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A tool's result. Content is captured with the same bounded retained-output format as commands.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Terminal outcome.
    pub status: ToolStatus,
    /// Short user-visible summary.
    pub summary: String,
    /// Model-visible content; the SDK persists a bounded first view and retained-output reference.
    pub content: String,
    /// Optional structured result; oversized values are retained in the output instead of the journal.
    pub data: Option<Value>,
    /// True only when known resources could not be cleaned up; makes cancel/close fail.
    pub cleanup_incomplete: bool,
}

impl ToolOutput {
    /// Return successful text without additional structured data.
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            status: ToolStatus::Ok,
            summary: "completed".into(),
            content: content.into(),
            data: None,
            cleanup_incomplete: false,
        }
    }
}

/// A host tool executed through the same authorization, persistence, ordering and cleanup slots.
pub trait ToolAdapter: std::fmt::Debug + Send + Sync {
    /// Supply a definition once when the runtime opens.
    fn definition(&self) -> ToolSpec;

    /// Execute final authorized arguments. On cancellation, finish cleanup before returning.
    fn execute(
        &self,
        action: ToolAuthorization,
        cancellation: ToolCancellation,
    ) -> AdapterFuture<'_, ToolOutput>;
}

/// Effective model and tool selection, inspectable without starting a model or tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSelection {
    /// Fully resolved request destination.
    #[serde(default)]
    pub request_url: String,
    /// Selected named connection.
    pub connection: String,
    /// Selected wire protocol.
    pub protocol: ProviderFormat,
    /// Selected model identifier.
    pub model: String,
    /// Configured context window; serialized request bytes have an additional hard limit.
    pub context_window_tokens: u32,
    /// Configured output limit sent with the request, if any.
    pub max_output_tokens: Option<u32>,
    /// Requested and effective reasoning/service settings.
    pub settings: ProviderSettings,
    /// Declared provider capabilities.
    pub capabilities: ProviderCapabilities,
    /// Media requested by the Agent or inherited from the route.
    #[serde(default)]
    pub output: RequestedOutput,
    /// Final tool selection after preset and explicit overrides.
    pub tools: Vec<crate::context::ToolSchema>,
}

/// Effective model settings available to a host before creating an Agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSelection {
    /// Fully resolved request destination.
    #[serde(default)]
    pub request_url: String,
    /// Selected named connection.
    pub connection: String,
    /// Selected wire protocol.
    pub protocol: ProviderFormat,
    /// Selected model identifier.
    pub model: String,
    /// Configured context window; requests have a separate serialized byte limit.
    pub context_window_tokens: u32,
    /// Configured output limit sent with the request, if any.
    pub max_output_tokens: Option<u32>,
    /// Requested and effective reasoning and service settings.
    pub settings: ProviderSettings,
    /// Declared capabilities of the selected route.
    pub capabilities: ProviderCapabilities,
    /// Media requested by the Agent or inherited from the route.
    #[serde(default)]
    pub output: RequestedOutput,
}

/// Inspect a draft connection's model capabilities without resolving its credential or sending a request.
pub fn inspect_connection_model(
    connection: &crate::config::ProviderConfig,
    model: &str,
) -> Result<ProviderCapabilities> {
    let endpoint = crate::config::resolve_request_url(
        connection.format,
        &connection.base_url,
        connection.options.endpoint_url.as_deref(),
    )?;
    Ok(crate::provider::capabilities::resolve_capabilities(
        &endpoint,
        model,
        crate::provider::types::ProviderKind::from(connection.format),
        &connection.options,
    ))
}

/// Inspect model settings available locally without dispatching work or resolving credentials.
pub fn inspect_model(
    config: &crate::config::Config,
    selection: &crate::session::AgentConfig,
) -> Result<ModelSelection> {
    let resolved = crate::session::runtime::resolve_agent_model(config, selection, None)?;
    Ok(ModelSelection {
        request_url: crate::provider::prepare::request_url(&resolved)
            .map_err(|failure| crate::Error::InvalidArgument(failure.message))?,
        connection: resolved.connection_name,
        protocol: protocol(resolved.kind),
        model: resolved.model,
        context_window_tokens: resolved.max_context_tokens,
        max_output_tokens: resolved.max_output_tokens,
        settings: resolved.settings,
        capabilities: resolved.capabilities,
        output: resolved.output,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct RegisteredTool {
    pub(crate) spec: ToolSpec,
    pub(crate) adapter: Arc<dyn ToolAdapter>,
}

pub(crate) fn protocol(kind: crate::provider::types::ProviderKind) -> ProviderFormat {
    kind.format()
}
