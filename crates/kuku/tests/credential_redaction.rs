use kuku::config::{load_config, SecretString, StoredCredential};

#[test]
fn direct_credential_debug_redacts_value() {
    let secret = SecretString::new("direct-secret");
    let credential = StoredCredential::DirectValue(secret);

    let debug = format!("{credential:?}");

    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("direct-secret"));
}

#[test]
fn environment_credential_debug_exposes_only_reference() {
    let credential = StoredCredential::EnvironmentReference("HOME".to_string());

    let debug = format!("{credential:?}");

    assert!(debug.contains("EnvironmentReference"));
    assert!(debug.contains("HOME"));
}

#[test]
fn nested_debug_redacts_direct_credential_value() {
    #[derive(Debug)]
    struct NestedCredential {
        credential: StoredCredential,
    }

    let nested = NestedCredential {
        credential: StoredCredential::DirectValue(SecretString::new("nested-secret")),
    };
    assert!(matches!(
        &nested.credential,
        StoredCredential::DirectValue(_)
    ));

    let debug = format!("{nested:?}");

    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("nested-secret"));
}

#[test]
fn tagged_credentials_preserve_direct_and_environment_sources_across_reopen() {
    let home = std::env::var("HOME").expect("HOME must be set for this test");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
default_model = "balanced"

[model.strong]
provider = "direct"
model = "test-model"

[model.balanced]
provider = "direct"
model = "test-model"

[model.light]
provider = "direct"
model = "test-model"

[provider.direct]
format = "anthropic"
base_url = "https://example.com"
credential = { source = "direct_value", value = "$HOME" }

[provider.environment]
format = "openai-responses"
base_url = "https://example.com"
credential = { source = "environment_reference", value = "HOME" }
"#,
    )
    .unwrap();

    let loaded = load_config(&path).unwrap();
    std::fs::write(&path, toml::to_string(&loaded).unwrap()).unwrap();
    let reopened = load_config(&path).unwrap().resolve().unwrap();

    let direct = reopened
        .provider("direct")
        .unwrap()
        .credential
        .resolve()
        .unwrap();
    let environment = reopened
        .provider("environment")
        .unwrap()
        .credential
        .resolve()
        .unwrap();

    assert_eq!("$HOME", direct.expose());
    assert_eq!(home, environment.expose());
}

#[test]
fn legacy_credential_field_is_rejected_even_with_tagged_credential() {
    let legacy_field = ["api", "key"].join("_");
    let config = format!(
        r#"
[provider.anthropic]
format = "anthropic"
base_url = "https://example.com"
credential = {{ source = "direct_value", value = "direct-secret" }}
{legacy_field} = "legacy-secret"
"#
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config).unwrap();

    let error = load_config(&path).unwrap_err();

    assert!(error.to_string().contains("unknown field"));
    assert!(!error.to_string().contains("legacy-secret"));
    assert!(!error.to_string().contains("direct-secret"));
}
