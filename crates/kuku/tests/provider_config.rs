mod config {
    pub use kuku::config::{
        ApiKey, Config, DiscoveryConfig, HandoffConfig, LogsConfig, PluginConfig, ProviderConfig,
        ProviderFormat, ProviderOptions, ProviderProfile, ResolvedThinking, ThinkLevel, TierConfig,
        UpdateConfig,
    };
}

mod context {
    pub use kuku::context::{CanonicalMessage, ContextAssembly};
}

mod prompt {
    pub use kuku::prompt::PromptCatalog;
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
    pub mod capabilities {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/provider/capabilities.rs"
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

use config::{ApiKey, Config, ProviderConfig, ThinkLevel, TierConfig};
use kuku::Error;
use provider::config::{resolve_config, ResolveConfigInput};
use provider::types::{Provider, ProviderKind};

fn default_config() -> Config {
    let mut tiers = BTreeMap::new();
    tiers.insert(
        "balanced".to_string(),
        TierConfig {
            reasoning_effort: None,
            service_tier: None,
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
            options: kuku::config::ProviderOptions {
                profile: kuku::config::ProviderProfile::Anthropic,
                reasoning_efforts: Some(vec![
                    "none".into(),
                    "low".into(),
                    "medium".into(),
                    "high".into(),
                ]),
                ..Default::default()
            },
            format: config::ProviderFormat::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            api_key: ApiKey::Plaintext("sk-ant-config".to_string()),
        },
    );

    Config {
        tiers,
        providers,
        default_tier: "balanced".to_string(),
        discovery: config::DiscoveryConfig::default(),
        handoff: config::HandoffConfig::default(),
        logs: config::LogsConfig::default(),
        plugin: config::PluginConfig::default(),
        update: config::UpdateConfig::default(),
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
    assert_eq!(resolved.api_key.resolve().unwrap(), "builder-key");
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
    assert_eq!(resolved.api_key.resolve().unwrap(), "sk-ant-config");
}

#[test]
fn selecting_connection_and_settings_does_not_resolve_its_credentials() {
    let mut cfg = default_config();
    let missing = format!("KUKU_PROVIDER_UNSET_CREDENTIAL_{}", std::process::id());
    assert!(std::env::var_os(&missing).is_none());
    cfg.providers.get_mut("anthropic").unwrap().api_key = ApiKey::Env(missing.clone());
    let resolved = resolve_config(ResolveConfigInput {
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();
    assert_eq!("anthropic", resolved.connection_name);
    assert_eq!("claude-sonnet-4-6", resolved.model);
    assert!(resolved.api_key.resolve().is_err());
    let debug = format!("{resolved:?}");
    assert!(!debug.contains(&missing));
    assert!(debug.contains("<redacted>"));
}

#[test]
fn explicit_tier_selects_different_tier() {
    let mut tiers = BTreeMap::new();
    tiers.insert(
        "balanced".to_string(),
        TierConfig {
            reasoning_effort: None,
            service_tier: None,
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
            reasoning_effort: None,
            service_tier: None,
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5-20251001".to_string(),
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
            options: kuku::config::ProviderOptions {
                profile: kuku::config::ProviderProfile::Anthropic,
                reasoning_efforts: Some(vec![
                    "none".into(),
                    "low".into(),
                    "medium".into(),
                    "high".into(),
                ]),
                ..Default::default()
            },
            format: config::ProviderFormat::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            api_key: ApiKey::Plaintext("sk-ant-config".to_string()),
        },
    );

    let cfg = Config {
        tiers,
        providers,
        default_tier: "balanced".to_string(),
        discovery: config::DiscoveryConfig::default(),
        handoff: config::HandoffConfig::default(),
        logs: config::LogsConfig::default(),
        plugin: config::PluginConfig::default(),
        update: config::UpdateConfig::default(),
    };

    let resolved = resolve_config(ResolveConfigInput {
        tier: Some("light".to_string()),
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.model, "claude-haiku-4-5-20251001");
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
    assert_eq!(resolved.api_key.resolve().unwrap(), "builder-key");
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

    assert_eq!(resolved.api_key.resolve().unwrap(), "sk-ant-config");
    assert_eq!(resolved.base_url, "https://api.anthropic.com");
}

#[test]
fn named_connection_selects_exact_gateway_with_the_same_protocol() {
    let mut cfg = default_config();
    let mut second = cfg.providers["anthropic"].clone();
    second.base_url = "https://second.example".into();
    second.api_key = ApiKey::Plaintext("second-key".into());
    cfg.providers.insert("second".into(), second);
    let original = cfg.clone();

    let resolved = resolve_config(ResolveConfigInput {
        connection: Some("second".into()),
        config: Some(cfg.clone()),
        ..Default::default()
    })
    .unwrap();

    assert_eq!("second", resolved.connection_name);
    assert_eq!("https://second.example", resolved.base_url);
    assert_eq!("second-key", resolved.api_key.resolve().unwrap());
    assert_eq!(original, cfg);
}

#[test]
fn unknown_connection_and_mismatched_protocol_are_rejected() {
    for input in [
        ResolveConfigInput {
            connection: Some("missing".into()),
            ..Default::default()
        },
        ResolveConfigInput {
            connection: Some("anthropic".into()),
            provider: Some(Provider::OpenAiResponses),
            ..Default::default()
        },
    ] {
        let error = resolve_config(ResolveConfigInput {
            config: Some(default_config()),
            ..input
        })
        .unwrap_err();
        assert!(matches!(error, Error::MissingProviderConfig(_)));
    }
}

#[test]
fn explicit_native_settings_are_validated_without_changing_reasoning() {
    let mut cfg = default_config();
    let connection = cfg.providers.get_mut("anthropic").unwrap();
    connection.options.service_tiers = Some(vec!["priority".into()]);
    let resolved = resolve_config(ResolveConfigInput {
        connection: Some("anthropic".into()),
        reasoning_effort: Some("low".into()),
        service_tier: Some("priority".into()),
        config: Some(cfg.clone()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(Some("low"), resolved.settings.reasoning_effort.as_deref());
    assert_eq!(Some("priority"), resolved.settings.service_tier.as_deref());
    for (reasoning, service) in [(Some("ultrafast"), None), (None, Some("ultrafast"))] {
        let result = resolve_config(ResolveConfigInput {
            reasoning_effort: reasoning.map(str::to_string),
            service_tier: service.map(str::to_string),
            config: Some(cfg.clone()),
            ..Default::default()
        });
        assert!(matches!(result, Err(Error::MissingProviderConfig(_))));
    }
}

#[test]
fn deepseek_tier_mapping_is_separate_from_explicit_native_settings() {
    let mut cfg = default_config();
    let tier = cfg.tiers.get_mut("balanced").unwrap();
    tier.model = "deepseek-flash".into();
    let connection = cfg.providers.get_mut("anthropic").unwrap();
    connection.base_url = "https://api.deepseek.com/anthropic".into();
    connection.options = Default::default();
    let resolved = resolve_config(ResolveConfigInput {
        config: Some(cfg.clone()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        Some("medium"),
        resolved.settings.requested_reasoning_effort.as_deref()
    );
    assert_eq!(Some("high"), resolved.settings.reasoning_effort.as_deref());
    for input in [
        ResolveConfigInput {
            reasoning_effort: Some("ultra".into()),
            ..Default::default()
        },
        ResolveConfigInput {
            service_tier: Some("priority".into()),
            ..Default::default()
        },
    ] {
        assert!(resolve_config(ResolveConfigInput {
            config: Some(cfg.clone()),
            ..input
        })
        .is_err());
    }
}

#[test]
fn connection_options_parse_and_programmatic_defaults_agree() {
    let connection: kuku::config::ProviderEntry = toml::from_str(
        r#"
format = "anthropic"
base_url = "https://openrouter.ai/api/v1"
api_key = "$UNRESOLVED_OFFLINE_KEY"
auth = "bearer"
profile = "openrouter"
reasoning_efforts = ["low"]
[routing]
order = ["wafer/us"]
allow_fallbacks = false
[routing.max_price]
prompt = 0.3
completion = 1.2
"#,
    )
    .unwrap();
    assert_eq!(
        Some(kuku::config::ProviderAuth::Bearer),
        connection.options.auth
    );
    assert_eq!(
        kuku::config::ProviderProfile::OpenRouter,
        connection.options.profile
    );
    let routing = connection.options.routing.unwrap();
    assert_eq!(vec!["wafer/us"], routing.order);
    assert_eq!(Some(false), routing.allow_fallbacks);
    assert!(routing.require_parameters);
    assert!(kuku::config::OpenRouterRouting::default().require_parameters);
}
