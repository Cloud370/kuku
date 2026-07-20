#[doc(hidden)]
pub mod context;

pub mod agent;
pub mod config;
pub mod conversation;
pub(crate) mod discovery;
pub mod error;
pub mod event;
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
#[doc(hidden)]
pub mod tool;
pub mod util;
pub mod wire;

pub use error::{Error, Result};
pub use event::{
    ConversationId, Cursor, ExecutionScope, InteractionId, RequestId, RequestScope,
    ReviewSubmissionId, RevisionToken, RunId, StorageExhaustionError, TaskId, TaskRevision,
    TaskState, TurnId, WorkspaceId,
};
pub use provider::types::ProviderFailureKind;
pub use provider::{Provider, ProviderUsage};
pub use query::{
    query, PermissionChoice, PermissionRequest, Query, Run, RunOutput, TaskQueryContext, ToolEvent,
    ToolKind, ToolSummary, UiEvent, WorkspaceCommandCancellation, WorkspaceCommandEvent,
    WorkspaceCommandOutput, WorkspaceCommandRequest, WorkspaceEntry, WorkspaceQueryCapability,
};
pub use session::{delete_session, list_sessions, SessionStatus, SessionSummary};

#[cfg(test)]
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
