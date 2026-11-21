mod endpoint;
mod model_capabilities;
mod mutate;
mod pricing;

pub use endpoint::{resolve_models_url, resolve_request_url};
pub use model_capabilities::{
    MediaAdapterSupport, ModelCapabilityDeclaration, ModelCapabilityEntry,
    ProtocolCapabilityDeclaration, RequestedOutput,
};
mod provider;
mod resolve;
pub(crate) mod types;

pub use mutate::{config_patch_defaults, generate_default, load_and_patch_config, set_value};
pub use pricing::{ModelPricing, TokenRates};
pub use provider::{
    OpenRouterPrice, OpenRouterRouting, PromptCacheScope, ProviderAuth, ProviderOptions,
    ProviderProfile, ProviderProxy, ProxyAuth,
};
pub use resolve::{load_config, parse_config_text, parse_config_text_with_env, show_redacted};
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
