mod mutate;
mod provider;
mod resolve;
pub(crate) mod types;

pub use mutate::{config_patch_defaults, generate_default, load_and_patch_config, set_value};
pub use provider::{
    OpenRouterPrice, OpenRouterRouting, ProviderAuth, ProviderOptions, ProviderProfile,
};
pub use resolve::{load_config, parse_config_text, show_redacted};
pub use types::{
    ApiKey, Config, ConfigFile, DiscoveryConfig, HandoffConfig, LogsConfig, ModelEntry,
    PluginConfig, ProviderConfig, ProviderEntry, ProviderFormat, ResolvedThinking, ThinkLevel,
    TierConfig, TierInfo, UpdateConfig,
};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "file_key_tests.rs"]
mod file_key_tests;
