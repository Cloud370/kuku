mod adapters;
mod budget;
mod frozen;
mod handoff;
mod helpers;
mod lifecycle;
mod provider;
mod recovery;
mod request;
mod run;
mod runtime_tools;
pub(crate) mod slots;
mod start;
mod step;
mod tool_exec;
mod tool_response;
mod types;
mod workspace_script;

pub(crate) use runtime_tools::definitions as runtime_tool_definitions;
pub(crate) use workspace_script::cancelled_deferred_script;

pub use types::{
    ModelPhase, ModelRecoveryInfo, PermissionChoice, PermissionRequest, Query, Run, RunOutput,
    ToolEvent, ToolKind, ToolSummary, UiEvent,
};

/// Start building a new query for the given prompt.
pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}
