mod handoff;
mod helpers;
mod lifecycle;
pub(crate) mod provider;
mod recovery;
mod request;
mod run;
pub(crate) mod slots;
mod start;
mod step;
mod tool_exec;
mod tool_response;
mod types;
mod workspace;

pub use types::{
    ModelRecoveryInfo, PermissionChoice, PermissionRequest, Query, Run, RunOutput, ToolEvent,
    ToolKind, ToolSummary, UiEvent,
};
pub use workspace::{
    is_default_excluded_workspace_directory, TaskQueryContext, WorkspaceCommandCancellation,
    WorkspaceCommandEvent, WorkspaceCommandOutput, WorkspaceCommandRequest, WorkspaceEntry,
    WorkspaceQueryCapability,
};

/// Start building a new query for the given prompt.
pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}
