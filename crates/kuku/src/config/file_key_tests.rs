use std::fs;
use std::io::Write;
use std::path::Path;

use super::{load_config, show_redacted, ApiKey};
use crate::util::private_directory;

const CONFIG: &str = r#"
default_model = "balanced"

[model.balanced]
provider = "local"
model = "fixture-model"

[model.strong]
provider = "local"
model = "fixture-model"

[model.light]
provider = "local"
model = "fixture-model"

[provider.local]
format = "openai-chat"
base_url = "https://provider.invalid/v1"
api_key = "$FIXTURE_KEY"
"#;

fn config_with_key(path: &Path, reference: &str) {
    let mut document: toml_edit::DocumentMut = CONFIG.parse().unwrap();
    document["provider"]["local"]["api_key"] = toml_edit::value(reference);
    fs::write(path, document.to_string()).unwrap();
}

fn private_key(directory: &Path, contents: &[u8]) -> std::path::PathBuf {
    let credentials = directory.join("credentials");
    private_directory(&credentials).unwrap();
    let mut file = tempfile::NamedTempFile::new_in(credentials).unwrap();
    file.write_all(contents).unwrap();
    file.as_file().sync_all().unwrap();
    #[cfg(windows)]
    crate::util::set_config_temp_security(None, file.path(), file.as_file()).unwrap();
    let (_, path) = file.keep().unwrap();
    path
}

#[test]
fn file_key_reference_resolves_only_the_explicit_private_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = private_key(directory.path(), b"fixture-only-not-a-real-key");
    let config_path = directory.path().join("config.toml");
    config_with_key(&config_path, &format!("file:{}", path.display()));

    let config = load_config(&config_path).unwrap().resolve().unwrap();
    assert!(config.providers["local"].api_key.resolve().unwrap() == "fixture-only-not-a-real-key");
    assert!(matches!(
        &config.providers["local"].api_key,
        ApiKey::File(file) if file == &path
    ));
}

#[test]
fn file_key_reference_rejects_relative_paths_and_has_no_environment_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    config_with_key(&config_path, "file:relative/key");
    assert!(load_config(&config_path).unwrap().resolve().is_err());

    let missing_key_path = directory.path().join("missing-fixture-key");
    config_with_key(
        &config_path,
        &format!("file:{}", missing_key_path.display()),
    );
    let api_key = load_config(&config_path)
        .unwrap()
        .resolve()
        .unwrap()
        .providers["local"]
        .api_key
        .clone();
    assert!(api_key.resolve().is_err());

    let environment = ApiKey::Env("KUKU_ISSUE93_MISSING_ENV_ONLY".into());
    assert!(environment.resolve().is_err());
}

#[test]
fn secret_values_are_redacted_from_debug_and_config_display() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    config_with_key(&config_path, "fixture-only-not-a-real-key");

    let file = load_config(&config_path).unwrap();
    assert!(!format!("{file:?}").contains("fixture-only-not-a-real-key"));
    assert!(!format!(
        "{:?}",
        ApiKey::Plaintext("fixture-only-not-a-real-key".into())
    )
    .contains("fixture-only-not-a-real-key"));
    assert!(!show_redacted(&config_path)
        .unwrap()
        .contains("fixture-only-not-a-real-key"));
}

#[test]
fn file_config_display_shows_the_reference_without_reading_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    let reference = format!("file:{}", directory.path().join("not-created").display());
    config_with_key(&config_path, &reference);

    let output = show_redacted(&config_path).unwrap();
    assert!(output.contains(&reference));
    assert!(output.contains("api_key"));
}

#[test]
fn credential_file_errors_do_not_include_file_contents() {
    let directory = tempfile::tempdir().unwrap();
    let path = private_key(directory.path(), b"fixture-only-not-a-real-key\n");
    let config_path = directory.path().join("config.toml");
    config_with_key(&config_path, &format!("file:{}", path.display()));

    let api_key = load_config(&config_path)
        .unwrap()
        .resolve()
        .unwrap()
        .providers["local"]
        .api_key
        .clone();
    let error = api_key.resolve().unwrap_err().to_string();
    assert!(!error.contains("fixture-only-not-a-real-key"));
}
