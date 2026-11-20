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
#[cfg(feature = "test_support")]
pub mod test_support;
pub use prompt::{builtin_prompt_catalog, PromptCatalog};
pub(crate) mod provider;
pub mod query;
pub mod session;
pub mod skill;
pub(crate) mod tool;
pub mod util;
pub mod wire;

pub use session::runtime::{WorkspaceExecutionHost, WorkspaceExecutionMetrics};
pub use tool::workspace_script::{
    WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptFacts,
    WorkspaceScriptLimits, WorkspaceScriptOperation, WorkspaceScriptResultKind,
};

pub use adapter::{
    AdapterFuture, AgentSelection, ModelAdapter, ModelEvent, ModelRequest, ModelStream, ToolAccess,
    ToolAdapter, ToolCancellation, ToolOutput, ToolSpec, ToolStatus,
};
pub use error::{Error, Result};
pub use execution::OutputReference;
pub use provider::types::ProviderFailureKind;
pub use provider::{
    Provider, ProviderCapabilities, ProviderReport, ProviderSettings, ProviderUsage,
};
pub use query::{
    query, ModelRecoveryInfo, PermissionChoice, PermissionRequest, Query, Run, RunOutput,
    ToolEvent, ToolKind, ToolSummary, UiEvent,
};
pub use session::{delete_session, list_sessions, SessionStatus, SessionSummary};
pub use session::{
    Activation, AgentConfig, AgentSnapshot, AgentStatus, AgentView, Input, InputSource,
    InputStatus, InputTiming, JobResult, JobSnapshot, JobStatus, ModelUsageSample, NamedValue,
    NamedValueKind, NamedValueSource, Observation, OutputChunk, OutputRead, OutputSegment,
    OutputStream, PendingInput, QuestionRequest, Receipt, ReplyPreview, RuntimeLimits, Session,
    SessionObserver, SessionOptions, TurnSummary, UsageTotals, Withdrawal, MAX_INPUT_VALUES,
    MAX_VALUE_NAME_BYTES,
};

#[cfg(test)]
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
