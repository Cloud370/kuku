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

pub use types::{
    PermissionChoice, PermissionRequest, Query, Run, RunOutput, TaskQueryContext, ToolEvent,
    ToolKind, ToolSummary, UiEvent, WorkspaceQueryCapability,
};

/// Start building a new query for the given prompt.
pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}
