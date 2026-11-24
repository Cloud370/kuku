mod config {
    pub use kuku::config::{
        resolve_request_url, ApiKey, Config, DiscoveryConfig, HandoffConfig, LogsConfig,
        MediaAdapterSupport, ModelCapabilityDeclaration, ModelCapabilityEntry, ModelThinkingTest,
        PluginConfig, ProtocolCapabilityDeclaration, ProviderConfig, ProviderFormat,
        ProviderOptions, ProviderProfile, RequestedOutput, ResolvedThinking, SubscriptionChannel,
        ThinkLevel, TierConfig, UpdateConfig,
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

use config::{ApiKey, Config, ProviderConfig, ProviderFormat, ThinkLevel, TierConfig};
use kuku::adapter::inspect_model;
use kuku::session::AgentConfig;
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
            max_output_tokens: Some(48_000),
            purpose: "balanced".to_string(),
            output: Default::default(),
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
fn generated_tiers_keep_known_anthropic_settings() {
    let config = toml::from_str::<kuku::config::ConfigFile>(kuku::config::generate_default())
        .unwrap()
        .resolve()
        .unwrap();
    for (tier, model, requested, effective) in [
        ("strong", "claude-sonnet-4-6", "high", "high"),
        ("balanced", "claude-sonnet-4-6", "medium", "medium"),
        ("light", "claude-haiku-4-5-20251001", "off", "none"),
    ] {
        let resolved = inspect_model(
            &config,
            &AgentConfig {
                tier: Some(tier.into()),
                ..AgentConfig::default()
            },
        )
        .unwrap();
        assert_eq!(model, resolved.model);
        assert_eq!(
            Some(requested),
            resolved.settings.requested_reasoning_effort.as_deref()
        );
        assert_eq!(
            Some(effective),
            resolved.settings.reasoning_effort.as_deref()
        );
    }
}

#[test]
fn unsupported_portable_thinking_preserves_the_request_without_a_native_value() {
    let mut cfg = default_config();
    for (format, url, model, think, native) in [
        (
            ProviderFormat::Anthropic,
            "https://gateway.example",
            "claude-haiku-4-5-20251001",
            ThinkLevel::Off,
            "none",
        ),
        (
            ProviderFormat::Anthropic,
            "https://gateway.example",
            "claude-sonnet-4-6",
            ThinkLevel::High,
            "high",
        ),
        (
            ProviderFormat::OpenAiChat,
            "https://api.openai.com/v1",
            "gpt-5",
            ThinkLevel::Off,
            "none",
        ),
        (
            ProviderFormat::OpenAiResponses,
            "https://api.openai.com/v1",
            "gpt-5",
            ThinkLevel::Off,
            "none",
        ),
    ] {
        let connection = cfg.providers.get_mut("anthropic").unwrap();
        connection.format = format;
        connection.base_url = url.into();
        connection.options = Default::default();
        let tier = cfg.tiers.get_mut("balanced").unwrap();
        tier.model = model.into();
        tier.think = think;

        let resolved = inspect_model(&cfg, &AgentConfig::default()).unwrap();
        assert_eq!(model, resolved.model);
        assert_eq!(
            Some(think.as_str()),
            resolved.settings.requested_reasoning_effort.as_deref()
        );
        assert_eq!(None, resolved.settings.reasoning_effort);
        assert!(!resolved
            .capabilities
            .reasoning_efforts
            .iter()
            .any(|effort| effort == native));

        let error = inspect_model(
            &cfg,
            &AgentConfig {
                reasoning_effort: Some(native.into()),
                ..AgentConfig::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::MissingProviderConfig(message) if message.contains(&format!("unsupported reasoning_effort '{native}'")))
        );
    }
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
            max_output_tokens: Some(48_000),
            purpose: "balanced".to_string(),
            output: Default::default(),
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
            max_output_tokens: Some(32_000),
            purpose: "light".to_string(),
            output: Default::default(),
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
    assert_eq!(Some(32_000), resolved.max_output_tokens);
}

#[test]
fn host_ceiling_bounds_explicit_agent_or_tier_output() {
    for (agent, host, expected) in [
        (None, None, 48_000),
        (Some(64_000), None, 64_000),
        (Some(200_000), None, 200_000),
        (Some(64_000), Some(2_048), 2_048),
    ] {
        let resolved = resolve_config(ResolveConfigInput {
            config: Some(default_config()),
            max_output_tokens: agent,
            max_output_tokens_per_request: host,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(Some(expected), resolved.max_output_tokens);
    }
}

#[test]
fn optional_protocol_does_not_inherit_an_unrelated_messages_presets_limit() {
    let mut config = default_config();
    config.providers.insert(
        "gateway".into(),
        ProviderConfig {
            format: ProviderFormat::OpenAiResponses,
            base_url: "https://gateway.example/v1".into(),
            api_key: ApiKey::Plaintext("unused".into()),
            options: Default::default(),
        },
    );
    let input = ResolveConfigInput {
        config: Some(config),
        connection: Some("gateway".into()),
        model: Some("new-model".into()),
        ..Default::default()
    };
    assert_eq!(
        None,
        resolve_config(input.clone()).unwrap().max_output_tokens
    );
    assert_eq!(
        Some(131_072),
        resolve_config(ResolveConfigInput {
            max_output_tokens: Some(131_072),
            ..input
        })
        .unwrap()
        .max_output_tokens
    );
}

#[test]
fn messages_with_unknown_capacity_requires_a_ceiling_unless_the_endpoint_supports_omission() {
    let input = ResolveConfigInput {
        provider: Some(Provider::Anthropic),
        model: Some("unknown-model".into()),
        base_url: Some("https://relay.example.test/v1".into()),
        api_key: Some("unused".into()),
        ..Default::default()
    };
    let error = resolve_config(input.clone()).unwrap_err();
    assert!(matches!(error, Error::MissingProviderConfig(message)
        if message.contains("max_output_tokens") && message.contains("unknown-model")));

    let resolved = resolve_config(ResolveConfigInput {
        max_output_tokens_per_request: Some(4_096),
        ..input.clone()
    })
    .unwrap();
    assert_eq!(Some(4_096), resolved.max_output_tokens);

    for base_url in [
        "https://api.deepseek.com/anthropic",
        "https://openrouter.ai/api/v1",
    ] {
        let resolved = resolve_config(ResolveConfigInput {
            base_url: Some(base_url.into()),
            ..input.clone()
        })
        .unwrap();
        assert_eq!(None, resolved.max_output_tokens);
    }
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
            reasoning_effort: Some("medium".into()),
            ..Default::default()
        },
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
    assert_eq!(
        kuku::config::PromptCacheScope::Workspace,
        connection.options.prompt_cache_scope
    );
    let routing = connection.options.routing.unwrap();
    assert_eq!(vec!["wafer/us"], routing.order);
    assert_eq!(Some(false), routing.allow_fallbacks);
    assert!(routing.require_parameters);
    assert!(kuku::config::OpenRouterRouting::default().require_parameters);
}

#[test]
fn prompt_cache_scope_config_parses_all_values_and_defaults_to_workspace() {
    for (value, expected) in [
        ("workspace", kuku::config::PromptCacheScope::Workspace),
        ("session", kuku::config::PromptCacheScope::Session),
        ("off", kuku::config::PromptCacheScope::Off),
    ] {
        let provider: kuku::config::ProviderEntry = toml::from_str(&format!(
            "format = \"openai-responses\"\nbase_url = \"https://api.openai.com/v1\"\napi_key = \"key\"\nprompt_cache_scope = \"{value}\""
        ))
        .unwrap();
        assert_eq!(expected, provider.options.prompt_cache_scope);
    }

    assert_eq!(
        kuku::config::PromptCacheScope::Workspace,
        kuku::config::ProviderOptions::default().prompt_cache_scope
    );
}
