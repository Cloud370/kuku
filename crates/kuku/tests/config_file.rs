mod common;

use kuku::config::{
    ApiKey, Config, ModelConfig, ModelRef, ProviderFormat, ThinkLevel, load_and_patch_config,
    load_config, parse_config_text_with_env,
};

fn config_from_toml(toml: &str) -> Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, toml).unwrap();
    let file = load_config(&path).unwrap();
    file.resolve().unwrap()
}

fn minimal_valid_toml() -> &'static str {
    r#"
default_model = { connection = "anthropic", model = "claude-sonnet-4-6" }

[model.anthropic."claude-sonnet-4-6"]
think = "medium"
context_window = 200000
max_output_tokens = 48000

[model.anthropic."claude-haiku-4-5-20251001"]
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "low-latency review"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-123"
"#
}

#[test]
fn full_config_round_trip() {
    let cfg = config_from_toml(minimal_valid_toml());

    assert_eq!(
        cfg.default_model(),
        &ModelRef::new("anthropic", "claude-sonnet-4-6")
    );
    let default = cfg.model(cfg.default_model()).unwrap();
    assert_eq!(default.think, ThinkLevel::Medium);
    assert_eq!(Some(48_000), default.max_output_tokens);

    let review = cfg
        .model(&ModelRef::new("anthropic", "claude-haiku-4-5-20251001"))
        .unwrap();
    assert_eq!(review.think, ThinkLevel::Off);
    assert_eq!(Some(32_000), review.max_output_tokens);
    assert_eq!("low-latency review", review.purpose);

    assert_eq!(
        cfg.provider("anthropic").unwrap().format.as_str(),
        "anthropic"
    );

    let display = cfg.redacted_display();
    assert!(!display.contains("sk-ant-123"));
    assert!(display.contains("<redacted>"));
    assert!(display.contains("max_output_tokens = 48000"));
}

#[test]
fn the_default_pair_is_registered_without_a_model_table() {
    let cfg = config_from_toml(
        r#"
default_model = { connection = "work", model = "model.with.version" }
[provider.work]
format = "openai-responses"
base_url = "https://gateway.example/v1"
api_key = "$UNUSED_FIXTURE_KEY"
"#,
    );
    assert_eq!(
        &ModelRef::new("work", "model.with.version"),
        cfg.default_model()
    );
    assert_eq!(cfg.models.len(), 1);
    assert_eq!(
        &ModelConfig::default(),
        cfg.model(cfg.default_model()).unwrap()
    );
}

#[test]
fn legacy_named_tier_toml_identifies_the_replacement() {
    let error = kuku::config::parse_config_text(
        r#"
default_model = "balanced"
[model.balanced]
provider = "work"
model = "fixture"
"#,
    )
    .unwrap_err();
    assert!(
        matches!(error, kuku::Error::ConfigField { ref field, .. } if field == "default_model")
    );
    assert!(error.to_string().contains("connection ="));
    assert!(
        error
            .to_string()
            .contains("named model tiers were replaced")
    );
}

#[test]
fn invalid_think_level_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "anthropic", model = "claude-sonnet-4-6" }

[model.anthropic."claude-sonnet-4-6"]
think = "extreme"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-123"
"#,
    )
    .unwrap();
    let file = load_config(&path).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(
        err.to_string()
            .contains(".think: value \"extreme\" must be")
    );
}

#[test]
fn api_key_env_ref_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "anthropic", model = "claude-sonnet-4-6" }

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
fn base_url_env_ref_resolves() {
    if !common::process::isolate_test(
        module_path!(),
        stringify!(base_url_env_ref_resolves),
        |command| {
            command.env("TEST_BASE_URL_VAR", "https://gateway.example/v1");
        },
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "anthropic", model = "claude-sonnet-4-6" }

[provider.anthropic]
format = "anthropic"
base_url = "$TEST_BASE_URL_VAR"
api_key = "sk-ant-123"
"#,
    )
    .unwrap();

    let file = load_config(&path).unwrap();
    let cfg = file.resolve().unwrap();

    assert_eq!(
        cfg.provider("anthropic").unwrap().base_url,
        "https://gateway.example/v1"
    );
}

#[test]
fn missing_base_url_env_ref_is_rejected() {
    if !common::process::isolate_test(
        module_path!(),
        stringify!(missing_base_url_env_ref_is_rejected),
        |command| {
            command.env_remove("MISSING_BASE_URL_VAR");
        },
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "anthropic", model = "claude-sonnet-4-6" }

[provider.anthropic]
format = "anthropic"
base_url = "$MISSING_BASE_URL_VAR"
api_key = "sk-ant-123"
"#,
    )
    .unwrap();

    let err = load_config(&path).unwrap_err();

    assert!(
        err.to_string()
            .contains("provider.anthropic.base_url: env var \"MISSING_BASE_URL_VAR\" is not set")
    );
}

#[test]
fn env_ref_resolves_before_toml_typed_deserialize() {
    if !common::process::isolate_test(
        module_path!(),
        stringify!(env_ref_resolves_before_toml_typed_deserialize),
        |command| {
            command.env("TEST_DEFAULT_MODEL", "provider/exact-1.2");
            command.env("TEST_PROVIDER_FORMAT", "anthropic");
        },
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "anthropic", model = "$TEST_DEFAULT_MODEL" }

[model.anthropic."$TEST_DEFAULT_MODEL"]
max_output_tokens = 1200

[provider.anthropic]
format = "$TEST_PROVIDER_FORMAT"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-123"
"#,
    )
    .unwrap();

    let file = load_config(&path).unwrap();
    let cfg = file.resolve().unwrap();

    assert_eq!(
        cfg.default_model(),
        &ModelRef::new("anthropic", "provider/exact-1.2")
    );
    assert_eq!(
        Some(1200),
        cfg.model(cfg.default_model()).unwrap().max_output_tokens
    );
    assert_eq!(
        cfg.provider("anthropic").unwrap().format,
        ProviderFormat::Anthropic
    );
}

#[test]
fn load_and_patch_config_resolves_env_refs_on_runtime_path() {
    if !common::process::isolate_test(
        module_path!(),
        stringify!(load_and_patch_config_resolves_env_refs_on_runtime_path),
        |command| {
            command.env("TEST_PROVIDER_FORMAT", "anthropic");
            command.env("TEST_BASE_URL_VAR", "https://gateway.example/v1");
            command.env("TEST_API_KEY_VAR", "runtime-key");
        },
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "anthropic", model = "claude-sonnet-4-6" }

[provider.anthropic]
format = "$TEST_PROVIDER_FORMAT"
base_url = "$TEST_BASE_URL_VAR"
api_key = "$TEST_API_KEY_VAR"
"#,
    )
    .unwrap();

    let file = load_and_patch_config(&path).unwrap();
    let cfg = file.resolve().unwrap();

    assert_eq!(cfg.default_model().connection, "anthropic");
    assert_eq!(
        cfg.provider("anthropic").unwrap().base_url,
        "https://gateway.example/v1"
    );
}

#[test]
fn unsupported_format_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "custom", model = "test" }

[provider.custom]
format = "grpc"
base_url = "https://custom.api"
api_key = "key"
"#,
    )
    .unwrap();
    let err = load_config(&path).unwrap_err();
    assert!(err.to_string().contains("grpc"));
}

#[test]
fn default_model_referencing_missing_connection_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = { connection = "nonexistent", model = "test" }

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
fn model_table_env_references_cannot_duplicate_an_exact_registration() {
    let error = parse_config_text_with_env(
        r#"
default_model = { connection = "work", model = "model.with.version" }
[model.work."$FIXTURE_ID"]
[model.work."model.with.version"]
"#,
        |name| {
            assert_eq!("FIXTURE_ID", name);
            Ok("model.with.version".into())
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("duplicates the registration"));
}
