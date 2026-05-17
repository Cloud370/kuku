mod config {
    pub use kuku::config::{
        load_config, ApiKey, Config, ProviderConfig, ResolvedThinking, ThinkLevel, TierConfig,
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

use config::{load_config, ApiKey, Config, ThinkLevel};
use provider::config::{resolve_config, ResolveConfigInput};

fn config_from_toml(toml: &str) -> Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, toml).unwrap();
    let file = load_config(&path).unwrap();
    file.resolve().unwrap()
}

fn minimal_valid_toml() -> &'static str {
    r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5"
think = "off"
context_window = 200000
max_output_tokens = 32000

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-123"
"#
}

#[test]
fn full_config_round_trip() {
    let cfg = config_from_toml(minimal_valid_toml());

    assert_eq!(cfg.default_tier(), "balanced");
    let strong = cfg.tier("strong").unwrap();
    assert_eq!(strong.provider, "anthropic");
    assert_eq!(strong.model, "claude-sonnet-4-6");
    assert_eq!(strong.think, ThinkLevel::High);
    assert_eq!(strong.context_window, 200_000);
    assert_eq!(strong.max_output_tokens, 64_000);
    assert_eq!(strong.purpose, "deep reasoning");

    let balanced = cfg.tier("balanced").unwrap();
    assert_eq!(balanced.think, ThinkLevel::Medium);
    assert_eq!(balanced.max_output_tokens, 48_000);

    let light = cfg.tier("light").unwrap();
    assert_eq!(light.think, ThinkLevel::Off);
    assert_eq!(light.max_output_tokens, 32_000);

    assert_eq!(cfg.provider("anthropic").unwrap().format, "anthropic");

    let display = cfg.redacted_display();
    assert!(!display.contains("sk-ant-123"));
    assert!(display.contains("<redacted>"));
}

#[test]
fn missing_required_tier_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-123"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err
        .to_string()
        .contains("required tier 'balanced' is missing"));
}

#[test]
fn invalid_think_level_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "extreme"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-123"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err.to_string().contains("think 'extreme' is invalid"));
}

#[test]
fn api_key_env_ref_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "$TEST_API_KEY_VAR"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let cfg = file.resolve().unwrap();
    let api_key = &cfg.provider("anthropic").unwrap().api_key;
    match api_key {
        ApiKey::Env(name) => assert_eq!(name, "TEST_API_KEY_VAR"),
        other => panic!("expected Env, got {other:?}"),
    }
}

#[test]
fn unsupported_format_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model.strong]
provider = "custom"
model = "test"

[model.balanced]
provider = "custom"
model = "test"

[model.light]
provider = "custom"
model = "test"

[provider.custom]
format = "grpc"
base_url = "https://custom.api"
api_key = "key"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err.to_string().contains("format 'grpc' is not supported"));
}

#[test]
fn tier_referencing_missing_provider_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model.strong]
provider = "nonexistent"
model = "test"

[model.balanced]
provider = "nonexistent"
model = "test"

[model.light]
provider = "nonexistent"
model = "test"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err.to_string().contains("not defined in [provider]"));
}

#[test]
fn redacted_display_never_leaks_api_keys() {
    let cfg = config_from_toml(minimal_valid_toml());
    let display = cfg.redacted_display();
    assert!(!display.contains("sk-ant-123"));
    assert!(display.contains("<redacted>"));
}

#[test]
fn resolution_chain_builder_model_wins() {
    let cfg = config_from_toml(minimal_valid_toml());

    let resolved = resolve_config(ResolveConfigInput {
        model: Some("claude-opus-4-7".to_string()),
        config: Some(cfg.clone()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(resolved.model, "claude-opus-4-7");

    let resolved = resolve_config(ResolveConfigInput {
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(resolved.model, "claude-sonnet-4-6");
    assert_eq!(resolved.api_key.expose(), "sk-ant-123");
}
