mod endpoint;
mod model_capabilities;
mod mutate;
mod pricing;

pub use endpoint::{resolve_models_url, resolve_request_url};
pub use model_capabilities::{
    CustomServiceTier, ImageInputSupport, MediaAdapterSupport, ModelCapabilityDeclaration,
    ModelCapabilityEntry, ModelThinkingTest, ProtocolCapabilityDeclaration, RequestedOutput,
};
mod provider;
mod request;
mod resolve;
pub(crate) mod subscription;
pub(crate) mod types;

pub use mutate::{
    config_patch_defaults, config_sha256, config_text_with_value, config_text_with_value_in_env,
    generate_default, load_and_patch_config, patch_config_text_with_env,
    patch_config_text_with_loaded_default, publish_config, set_value,
};
pub use pricing::{ConfiguredRouteRates, ModelPricing, PriceCatalog, RouteQuote, TokenRates};
pub use provider::{
    OpenRouterPrice, OpenRouterRouting, PromptCacheScope, ProviderAuth, ProviderOptions,
    ProviderProfile, ProviderProxy, ProxyAuth,
};
pub use request::RequestFields;
pub use resolve::{load_config, parse_config_text, parse_config_text_with_env, show_redacted};
pub use subscription::{SubscriptionAccount, SubscriptionChannel};
pub use types::{
    ApiKey, Config, ConfigFile, DiscoveryConfig, HandoffConfig, LogsConfig, ModelConfig,
    ModelEntry, ModelInfo, ModelRef, PluginConfig, ProviderConfig, ProviderEntry, ProviderFormat,
    ResolvedThinking, ThinkLevel, UpdateConfig,
};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "file_key_tests.rs"]
mod file_key_tests;

#[cfg(test)]
pub(crate) fn test_config() -> Config {
    parse_config_text(
        r#"
default_model = { connection = "test", model = "fixture" }
[provider.test]
format = "openai-chat"
base_url = "https://provider.invalid/v1"
api_key = "fixture-only"
"#,
    )
    .unwrap()
    .resolve()
    .unwrap()
}
