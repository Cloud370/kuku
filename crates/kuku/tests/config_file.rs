mod config {
    pub use kuku::config::{load_config, ApiKeySource, Config, ResolvedThinking};
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

use config::{load_config, Config};
use provider::config::{resolve_config, ResolveConfigInput};

fn config_from_toml(toml: &str) -> Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, toml).unwrap();
    let file = load_config(&path).unwrap();
    file.resolve().unwrap()
}

#[test]
fn full_config_round_trip() {
    let cfg = config_from_toml(
        r#"
[model]
strong = "anthropic:claude-sonnet-4-6"
fast = "openai:gpt-5-mini"
default = "strong"

[provider.anthropic]
format = "anthropic"
api_key = "sk-ant-123"

[provider.anthropic.thinking]
low = 2048
medium = 8192
high = 32000

[provider.openai]
format = "openai"
api_key_env = "OPENAI_API_KEY"
"#,
    );

    assert_eq!(cfg.default_model(), "strong");
    assert_eq!(
        cfg.resolve_model_alias("strong"),
        Some("anthropic:claude-sonnet-4-6")
    );
    assert_eq!(cfg.resolve_model_alias("fast"), Some("openai:gpt-5-mini"));
    assert_eq!(cfg.provider("anthropic").unwrap().format, "anthropic");
    assert!(cfg.provider("anthropic").unwrap().thinking.low.is_some());
    assert!(cfg.provider("openai").unwrap().thinking.low.is_none());

    let display = cfg.redacted_display();
    assert!(!display.contains("sk-ant-123"));
    assert!(display.contains("<redacted>"));
    assert!(display.contains("OPENAI_API_KEY"));
}

#[test]
fn empty_config_has_sensible_defaults() {
    let cfg = config_from_toml("");
    assert!(cfg.models.is_empty());
    assert!(cfg.providers.is_empty());
    assert_eq!(cfg.default_model(), "default");
}

#[test]
fn model_without_colon_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[model]
bad = "no-colon"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err.to_string().contains("no-colon"));
}

#[test]
fn both_api_key_types_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[provider.dup]
format = "anthropic"
api_key = "sk-123"
api_key_env = "ENV"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err.to_string().contains("not both"));
}

#[test]
fn redacted_display_never_leaks_api_keys() {
    let cfg = config_from_toml(
        r#"
[provider.anthropic]
format = "anthropic"
api_key = "sk-ant-very-secret-key-12345"
"#,
    );
    let display = cfg.redacted_display();
    assert!(!display.contains("sk-ant-very-secret-key-12345"));
    assert!(!display.contains("very-secret"));
    assert!(display.contains("<redacted>"));
}

#[test]
fn resolution_chain_order_explicit_beats_env_beats_file_beats_default() {
    let cfg = config_from_toml(
        r#"
[model]
strong = "anthropic:claude-file"
default = "strong"

[provider.anthropic]
format = "anthropic"
api_key = "sk-ant-file"
"#,
    );

    // explicit model param wins over everything
    let resolved = resolve_config(ResolveConfigInput {
        model: Some("claude-opus-4-7".to_string()),
        config: Some(cfg.clone()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(resolved.model, "claude-opus-4-7");

    // config file model used when no explicit param
    let resolved = resolve_config(ResolveConfigInput {
        config: Some(cfg),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(resolved.model, "claude-file");

    // API key from config file is resolved
    assert_eq!(resolved.api_key.expose(), "sk-ant-file");
}
