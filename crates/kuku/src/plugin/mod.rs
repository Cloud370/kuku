pub(crate) mod executor;
pub(crate) mod hook;
pub(crate) mod loader;
pub(crate) mod manifest;
pub(crate) mod matcher;
pub(crate) mod output;
pub(crate) mod registry;

/// Lifecycle event that triggers plugin hook execution.
pub use hook::HookEvent;
/// Central registry of loaded plugin packages, their hooks, and skill directories.
pub use registry::PluginRegistry;
