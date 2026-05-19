mod helpers;
mod run;
mod start;
mod step;
mod types;

pub(crate) use types::PermissionMode;
pub use types::{PermissionChoice, PermissionRequest, Query, Run, RunOutput, UiEvent};

/// Start building a new query for the given prompt.
pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}
