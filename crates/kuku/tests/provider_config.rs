mod config {
    pub use kuku::config::{
        ApiKey, Config, ProviderConfig, ResolvedThinking, ThinkLevel, TierConfig,
    };
}

mod context {
    pub use kuku::context::ContextAssembly;
}

mod error {
    pub use kuku::{Error, Result};
}

mod provider {
    #[allow(dead_code)]
    pub mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/types.rs"
        ));
    }

    #[allow(dead_code)]
    pub mod config {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/config.rs"
        ));
    }
}

mod common;

use std::collections::BTreeMap;

use config::{ApiKey, Config, ProviderConfig, TierConfig, ThinkLevel};
use kuku::Error;
use provider::config::{resolve_config, ResolveConfigInput};
use provider::types::{Provider, ProviderKind};

fn default_config() -> Config {
    let mut tiers = BTreeMap::new();
    tiers.insert(
        "balanced".to_string(),
        TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            think: ThinkLevel::Medium,
            context_window: 200_000,
            max_output_tokens: 48_000,
            purpose: "balanced".to_string(),
        },
    );

    let mut providers = BTreeMap::new();
    providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            format: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: ApiKey::Plaintext("sk-ant-config".to_string()),
        },
    );

    Config {
        tiers,
        providers,
        default_tier: "balanced".to_string(),
    }
}

#[test]
fn builder_values_override_config_values() {

    let cfg = default_config();

    let resolved = resolve_config(ResolveConfigInput {
        provider: Some(Provider::Anthropic),
        model: Some("claude-opus-4-7".to_string()),
        base_url: Some("https://builder.example".to_string()),
        api_key: Some("builder-key".to_string()),
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.kind, ProviderKind::Anthropic);
    assert_eq!(resolved.model, "claude-opus-4-7");
    assert_eq!(resolved.base_url, "https://builder.example");
    assert_eq!(resolved.api_key.expose(), "builder-key");
}

#[test]
fn tier_resolution_uses_config_defaults() {

    let cfg = default_config();

    let resolved = resolve_config(ResolveConfigInput {
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.kind, ProviderKind::Anthropic);
    assert_eq!(resolved.model, "claude-sonnet-4-6");
    assert_eq!(resolved.base_url, "https://api.anthropic.com");
    assert_eq!(resolved.api_key.expose(), "sk-ant-config");
}

#[test]
fn explicit_tier_selects_different_tier() {


    let mut tiers = BTreeMap::new();
    tiers.insert(
        "balanced".to_string(),
        TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            think: ThinkLevel::Medium,
            context_window: 200_000,
            max_output_tokens: 48_000,
            purpose: "balanced".to_string(),
        },
    );
    tiers.insert(
        "light".to_string(),
        TierConfig {
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5".to_string(),
            think: ThinkLevel::Off,
            context_window: 200_000,
            max_output_tokens: 32_000,
            purpose: "light".to_string(),
        },
    );

    let mut providers = BTreeMap::new();
    providers.insert(
        "anthropic".to_string(),
        ProviderConfig {
            format: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: ApiKey::Plaintext("sk-ant-config".to_string()),
        },
    );

    let cfg = Config {
        tiers,
        providers,
        default_tier: "balanced".to_string(),
    };

    let resolved = resolve_config(ResolveConfigInput {
        tier: Some("light".to_string()),
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.model, "claude-haiku-4-5");
    assert_eq!(resolved.think_level, ThinkLevel::Off);
    assert_eq!(resolved.max_output_tokens, 32_000);
}

#[test]
fn missing_config_returns_structured_error() {


    let error = resolve_config(ResolveConfigInput::default()).unwrap_err();

    assert!(matches!(error, Error::MissingProviderConfig(_)));
}

#[test]
fn builder_model_overrides_tier_model() {

    let cfg = default_config();

    let resolved = resolve_config(ResolveConfigInput {
        model: Some("claude-opus-4-7".to_string()),
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.model, "claude-opus-4-7");
}

#[test]
fn builder_values_override_all_config_tier_settings() {
    let cfg = default_config();

    let resolved = resolve_config(ResolveConfigInput {
        provider: Some(Provider::Anthropic),
        model: Some("claude-opus-4-7".to_string()),
        base_url: Some("https://custom-gateway.example".to_string()),
        api_key: Some("builder-key".to_string()),
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.kind, ProviderKind::Anthropic);
    assert_eq!(resolved.model, "claude-opus-4-7");
    assert_eq!(resolved.base_url, "https://custom-gateway.example");
    assert_eq!(resolved.api_key.expose(), "builder-key");
}

#[test]
fn nonexistent_tier_returns_error() {

    let cfg = default_config();

    let error = resolve_config(ResolveConfigInput {
        tier: Some("nonexistent".to_string()),
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap_err();

    assert!(matches!(error, Error::MissingProviderConfig(_)));
}

#[test]
fn config_api_key_and_base_url_are_used() {

    let cfg = default_config();

    let resolved = resolve_config(ResolveConfigInput {
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.api_key.expose(), "sk-ant-config");
    assert_eq!(resolved.base_url, "https://api.anthropic.com");
}
