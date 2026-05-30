mod mutate;
mod resolve;
pub(crate) mod types;

pub use mutate::{config_patch_defaults, generate_default, load_and_patch_config, set_value};
pub use resolve::{load_config, show_redacted};
pub use types::{
    ApiKey, Config, ConfigFile, DiscoveryConfig, HandoffConfig, ModelEntry, PluginConfig,
    ProviderConfig, ProviderEntry, ProviderFormat, ResolvedThinking, ThinkLevel, TierConfig,
    TierInfo,
};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
