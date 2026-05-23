#[doc(hidden)]
pub mod context;

pub mod config;
pub mod error;
pub mod event;
pub(crate) mod notice;
pub mod permission;
pub mod prompt;
pub use prompt::{builtin_prompt_catalog, PromptCatalog};
pub(crate) mod provider;
pub mod query;
pub mod session;
pub mod skill;
pub mod subagent;
pub(crate) mod tool;
pub mod wire;

pub use error::{Error, Result};
pub use provider::types::ProviderFailureKind;
pub use provider::{Provider, ProviderUsage};
pub use query::{
    query, PermissionChoice, PermissionRequest, Query, Run, RunOutput, ToolEvent, ToolKind, UiEvent,
};
pub use session::{list_sessions, SessionSummary};

#[cfg(test)]
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
