mod handoff;
mod helpers;
mod lifecycle;
mod provider;
mod run;
pub(crate) mod slots;
mod start;
mod step;
mod tool_exec;
mod types;

pub use types::{
    PermissionChoice, PermissionRequest, Query, Run, RunOutput, ToolEvent, ToolKind, ToolSummary,
    UiEvent,
};

/// Start building a new query for the given prompt.
pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}
