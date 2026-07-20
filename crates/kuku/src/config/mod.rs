mod mutate;
mod resolve;
mod secret;
pub(crate) mod types;

pub use mutate::{config_patch_defaults, generate_default, load_and_patch_config, set_value};
pub use resolve::{load_config, parse_config_file, show_redacted};
pub use secret::{SecretString, StoredCredential};
pub use types::{
    Config, ConfigFile, DiscoveryConfig, HandoffConfig, LogsConfig, ModelEntry, PluginConfig,
    ProviderConfig, ProviderEntry, ProviderFormat, ResolvedThinking, ThinkLevel, TierConfig,
    TierInfo, UpdateConfig,
};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
