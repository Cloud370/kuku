#[doc(hidden)]
pub mod context;

pub mod error;
pub mod event;
pub mod permission;
pub(crate) mod prompt;
pub(crate) mod provider;
pub mod query;
pub mod session;
pub(crate) mod tool;

pub use error::{Error, Result};
pub use provider::Provider;
pub use query::{query, Query, Run, RunOutput, UiEvent};
