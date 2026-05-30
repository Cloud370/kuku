pub(crate) mod executor;
pub(crate) mod hook;
pub(crate) mod loader;
pub(crate) mod manifest;
pub(crate) mod matcher;
pub(crate) mod output;
pub(crate) mod registry;

pub use hook::HookEvent;
pub use registry::PluginRegistry;
