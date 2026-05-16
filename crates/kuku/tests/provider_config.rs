mod config {
    pub use kuku::config::{ApiKeySource, Config, ResolvedThinking};
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

use common::env_lock;
use kuku::Error;
use provider::config::{
    resolve_config, ResolveConfigInput, ENV_ANTHROPIC_API_KEY, ENV_ANTHROPIC_BASE_URL,
    ENV_ANTHROPIC_MODEL, ENV_API_KEY, ENV_BASE_URL, ENV_MODEL, ENV_OPENAI_API_KEY,
    ENV_OPENAI_BASE_URL, ENV_OPENAI_MODEL, ENV_PROVIDER,
};
use provider::types::{Provider, ProviderKind};

fn clear_env() {
    for key in [
        ENV_PROVIDER,
        ENV_MODEL,
        ENV_BASE_URL,
        ENV_API_KEY,
        ENV_ANTHROPIC_MODEL,
        ENV_ANTHROPIC_BASE_URL,
        ENV_ANTHROPIC_API_KEY,
        ENV_OPENAI_MODEL,
        ENV_OPENAI_BASE_URL,
        ENV_OPENAI_API_KEY,
    ] {
        std::env::remove_var(key);
    }
}

#[test]
fn builder_values_override_env_values() {
    let _guard = env_lock().lock().unwrap();
    clear_env();
    std::env::set_var(ENV_PROVIDER, "openai-compatible");
    std::env::set_var(ENV_MODEL, "env-model");
    std::env::set_var(ENV_BASE_URL, "https://env.example/v1");
    std::env::set_var(ENV_API_KEY, "env-key");

    let resolved = resolve_config(ResolveConfigInput {
        provider: Some(Provider::Anthropic),
        model: Some("claude-sonnet-4-6".to_string()),
        base_url: Some("https://builder.example".to_string()),
        api_key: Some("builder-key".to_string()),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(resolved.kind, ProviderKind::Anthropic);
    assert_eq!(resolved.model, "claude-sonnet-4-6");
    assert_eq!(resolved.base_url, "https://builder.example");
    assert_eq!(resolved.api_key.expose(), "builder-key");
}

#[test]
fn provider_specific_env_overrides_generic_env() {
    let _guard = env_lock().lock().unwrap();
    clear_env();
    std::env::set_var(ENV_PROVIDER, "anthropic");
    std::env::set_var(ENV_MODEL, "generic-model");
    std::env::set_var(ENV_BASE_URL, "https://generic.example/v1");
    std::env::set_var(ENV_API_KEY, "generic-key");
    std::env::set_var(ENV_ANTHROPIC_MODEL, "claude-sonnet-4-6");
    std::env::set_var(ENV_ANTHROPIC_BASE_URL, "https://anthropic.example");
    std::env::set_var(ENV_ANTHROPIC_API_KEY, "anthropic-key");

    let resolved = resolve_config(ResolveConfigInput::default()).unwrap();

    assert_eq!(resolved.kind, ProviderKind::Anthropic);
    assert_eq!(resolved.model, "claude-sonnet-4-6");
    assert_eq!(resolved.base_url, "https://anthropic.example");
    assert_eq!(resolved.api_key.expose(), "anthropic-key");
}

#[test]
fn missing_config_returns_structured_error() {
    let _guard = env_lock().lock().unwrap();
    clear_env();

    let error = resolve_config(ResolveConfigInput::default()).unwrap_err();

    assert!(matches!(error, Error::MissingProviderConfig(_)));
}

#[test]
fn both_provider_specific_keys_without_provider_is_ambiguous() {
    let _guard = env_lock().lock().unwrap();
    clear_env();
    std::env::set_var(ENV_ANTHROPIC_API_KEY, "anthropic-key");
    std::env::set_var(ENV_OPENAI_API_KEY, "openai-key");

    let error = resolve_config(ResolveConfigInput::default()).unwrap_err();

    assert!(matches!(error, Error::AmbiguousProviderConfig(_)));
}

#[test]
fn single_provider_specific_key_autodetects_provider() {
    let _guard = env_lock().lock().unwrap();
    clear_env();
    std::env::set_var(ENV_OPENAI_API_KEY, "openai-key");
    std::env::set_var(ENV_OPENAI_MODEL, "gpt-5.4-mini");

    let resolved = resolve_config(ResolveConfigInput::default()).unwrap();

    assert_eq!(resolved.kind, ProviderKind::OpenAiCompatible);
    assert_eq!(resolved.model, "gpt-5.4-mini");
}

#[test]
fn built_in_default_base_url_is_provider_specific() {
    let _guard = env_lock().lock().unwrap();
    clear_env();
    std::env::set_var(ENV_PROVIDER, "anthropic");
    std::env::set_var(ENV_ANTHROPIC_MODEL, "claude-sonnet-4-6");
    std::env::set_var(ENV_ANTHROPIC_API_KEY, "anthropic-key");

    let resolved = resolve_config(ResolveConfigInput::default()).unwrap();

    assert_eq!(resolved.base_url, "https://api.anthropic.com");
}

#[test]
fn unknown_provider_env_value_is_rejected() {
    let _guard = env_lock().lock().unwrap();
    clear_env();
    std::env::set_var(ENV_PROVIDER, "mystery");

    let error = resolve_config(ResolveConfigInput::default()).unwrap_err();

    assert!(
        matches!(error, Error::MissingProviderConfig(message) if message.contains("unknown KUKU_PROVIDER value"))
    );
}

#[test]
fn resolves_from_env_vars_when_no_config_file() {
    let _guard = env_lock().lock().unwrap();
    clear_env();
    std::env::set_var(ENV_PROVIDER, "anthropic");
    std::env::set_var(ENV_ANTHROPIC_MODEL, "claude-sonnet-4-6");
    std::env::set_var(ENV_ANTHROPIC_API_KEY, "sk-ant-legacy");

    let resolved = resolve_config(ResolveConfigInput::default()).unwrap();
    assert_eq!(resolved.kind, ProviderKind::Anthropic);
    assert_eq!(resolved.model, "claude-sonnet-4-6");
    assert!(resolved.thinking.low.is_none());
}
