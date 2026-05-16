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

pub use error::{Error, Result};
pub use provider::Provider;
pub use query::{query, PermissionChoice, PermissionRequest, Query, Run, RunOutput, UiEvent};
pub use session::{list_sessions, SessionSummary};

#[cfg(test)]
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
