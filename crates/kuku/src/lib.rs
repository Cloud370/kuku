#[doc(hidden)]
pub mod context;

pub mod error;
pub mod event;
pub(crate) mod notice;
pub mod permission;
pub(crate) mod prompt;
pub(crate) mod provider;
pub mod query;
pub mod session;
pub(crate) mod tool;
pub mod view;

pub use error::{Error, Result};
pub use provider::Provider;
pub use query::{query, PermissionChoice, PermissionRequest, Query, Run, RunOutput, UiEvent};
pub use session::{list_sessions, SessionSummary};
pub use view::{derive_final_output, format_event_brief};
