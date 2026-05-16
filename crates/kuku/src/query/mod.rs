mod helpers;
mod run;
mod start;
mod step;
mod types;

pub use types::{PermissionChoice, PermissionRequest, Query, Run, RunOutput, UiEvent};

pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}
