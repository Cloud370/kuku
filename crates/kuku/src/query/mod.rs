mod handoff;
mod helpers;
mod lifecycle;
pub(crate) mod provider;
mod run;
pub(crate) mod slots;
mod start;
mod step;
mod tool_exec;
mod types;
mod workspace;

pub use types::{
    PermissionChoice, PermissionRequest, Query, Run, RunOutput, ToolEvent, ToolKind, ToolSummary,
    UiEvent,
};
pub use workspace::{
    TaskQueryContext, WorkspaceCommandCancellation, WorkspaceCommandEvent, WorkspaceCommandOutput,
    WorkspaceCommandRequest, WorkspaceEntry, WorkspaceQueryCapability,
};

/// Start building a new query for the given prompt.
pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}
