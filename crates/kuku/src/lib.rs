#[doc(hidden)]
pub mod context;

pub mod adapter;
pub mod agent;
pub mod config;
pub mod conversation;
pub(crate) mod discovery;
pub mod error;
pub mod event;
pub mod execution;
pub mod log;
pub(crate) mod notice;
pub mod permission;
pub mod plugin;
pub mod prompt;
#[cfg(any(test, feature = "test_support"))]
mod test_process;
#[cfg(feature = "test_support")]
pub mod test_support;
pub use prompt::{PromptCatalog, builtin_prompt_catalog};
pub(crate) mod provider;
pub mod query;
pub mod session;
pub mod skill;
pub(crate) mod tool;
pub mod util;
pub mod wire;

pub use session::runtime::{WorkspaceExecutionHost, WorkspaceExecutionMetrics, WorkspaceIoLimits};
pub use tool::run_js::{
    WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptFacts,
    WorkspaceScriptLimits, WorkspaceScriptOperation, WorkspaceScriptResultKind,
};

pub use adapter::{
    AdapterFuture, AgentSelection, ModelAdapter, ModelEvent, ModelRequest, ModelStream, ToolAccess,
    ToolAdapter, ToolCancellation, ToolOutput, ToolSpec, ToolStatus,
};
pub use config::ModelRef;
pub use error::{Error, Result};
pub use execution::OutputReference;
pub use provider::trace::ProviderTraceMode;
pub use provider::types::ProviderFailureKind;

/// Version and source revision embedded at build time.
pub const BUILD_VERSION: &str = env!("KUKU_VERSION");
/// Full source commit embedded at build time, or an empty string if unavailable.
pub const BUILD_REVISION: &str = env!("KUKU_BUILD_REVISION");
pub use provider::{
    CapabilitySource, DeclaredCapability, Provider, ProviderCapabilities, ProviderReport,
    ProviderSettings, ProviderUsage,
};
pub use query::{
    ModelRecoveryInfo, PermissionChoice, PermissionRequest, Query, Run, RunOutput, ToolEvent,
    ToolKind, ToolSummary, UiEvent, query,
};
pub use session::{
    Activation, AgentConfig, AgentSnapshot, AgentStatus, AgentView, Input, InputSource,
    InputStatus, InputTiming, JobResult, JobSnapshot, JobStatus, MAX_INPUT_VALUES,
    MAX_VALUE_NAME_BYTES, ModelUsageSample, NamedValue, NamedValueKind, NamedValueSource,
    Observation, OutputChunk, OutputRead, OutputSegment, OutputStream, PendingInput,
    QuestionRequest, Receipt, ReplyPreview, RuntimeLimits, Session, SessionObserver,
    SessionOptions, TurnSummary, UsageTotals, Withdrawal,
};
pub use session::{SessionStatus, SessionSummary, delete_session, list_sessions};
