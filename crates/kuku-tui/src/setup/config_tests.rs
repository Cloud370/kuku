use std::fs;
use std::io::Write;

use kuku::config::{generate_default, load_config, ApiKey};
use kuku::util::private_directory;
use kuku_client::ConnectionOptions;
use toml_edit::{value, DocumentMut};

use super::{save, save_with_hook, Settings};
use crate::setup::state::SetupState;
use crate::setup::CredentialSource;

fn options(root: &std::path::Path, config: Option<std::path::PathBuf>) -> ConnectionOptions {
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    ConnectionOptions {
        workspace: Some(workspace),
        data_dir: Some(root.join("service-data")),
        config,
        ..ConnectionOptions::default()
    }
}

#[tokio::test]
async fn existing_settings_preserve_comments_providers_tiers_and_connection_options() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let contents = generate_default().replace(
        "[provider.anthropic]",
        "[provider.anthropic]\n# preserved provider comment",
    );
    let mut document: DocumentMut = contents.parse().unwrap();
    document["provider"]["anthropic"]["base_url"] = value("https://custom.invalid/v1");
    document["provider"]["anthropic"]["profile"] = value("anthropic");
    document["model"]["balanced"]["model"] = value("custom-balanced");
    document["model"]["strong"]["model"] = value("preserve-strong");
    document["model"]["light"]["model"] = value("preserve-light");
    fs::write(&config, document.to_string()).unwrap();

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    assert!(
        matches!(state.choice(), crate::setup::ConnectionChoice::Existing(name) if name == "anthropic")
    );
    state.model.set_text("updated-balanced".into());
    save(&settings, &state).unwrap();

    let output = fs::read_to_string(config).unwrap();
    assert!(output.contains("# preserved provider comment"));
    assert!(output.contains("base_url = \"https://custom.invalid/v1\""));
    assert!(output.contains("profile = \"anthropic\""));
    let parsed = load_config(&settings.config_path).unwrap();
    assert_eq!("updated-balanced", parsed.model["balanced"].model);
    assert_eq!("preserve-strong", parsed.model["strong"].model);
    assert_eq!("preserve-light", parsed.model["light"].model);
    assert!(parsed.provider.contains_key("openai"));
}

#[tokio::test]
async fn new_configuration_keeps_the_three_default_tiers() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path(), None))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    state.model.set_text("fixture-model".into());
    save(&settings, &state).unwrap();

    let parsed = load_config(&settings.config_path).unwrap();
    for tier in ["strong", "balanced", "light"] {
        assert_eq!("fixture-model", parsed.model[tier].model);
        assert_eq!("anthropic", parsed.model[tier].provider);
    }
}

#[tokio::test]
async fn all_provider_presets_generate_parseable_configs() {
    for preset in crate::setup::ProviderPreset::ALL {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings::load(options(directory.path(), None))
            .await
            .unwrap();
        let mut state = SetupState::new(&settings);
        state.selected = state
            .choices
            .iter()
            .position(|choice| *choice == crate::setup::ConnectionChoice::Preset(preset))
            .unwrap();
        state.model.set_text(preset.default_model().into());
        state.environment.set_text(preset.default_env().into());
        save(&settings, &state).unwrap();

        let parsed = load_config(&settings.config_path).unwrap();
        assert!(parsed.model["balanced"].provider == preset.connection());
        assert!(parsed.model["balanced"].model == preset.default_model());
    }
}

#[tokio::test]
async fn invalid_model_and_environment_name_do_not_write_config() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path(), None))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);

    state.model.set_text("$MODEL".into());
    assert!(save(&settings, &state).is_err());
    state.model.set_text("valid-model".into());
    state.environment.set_text("NOT A NAME".into());
    assert!(save(&settings, &state).is_err());
    assert!(!settings.config_path.exists());
}

#[test]
fn stored_key_input_rejects_control_characters_without_echoing_them() {
    let error = super::super::state::valid_stored_key("fixture-key\twith-control").unwrap_err();
    assert!(!error.contains("fixture-key"));
}

fn private_key(directory: &std::path::Path, contents: &[u8]) -> std::path::PathBuf {
    fs::create_dir_all(directory.parent().unwrap()).unwrap();
    private_directory(directory).unwrap();
    let mut file = tempfile::NamedTempFile::new_in(directory).unwrap();
    file.write_all(contents).unwrap();
    file.as_file().sync_all().unwrap();
    file.keep().unwrap().1
}

fn config_with_file_key(config: &std::path::Path, path: &std::path::Path) {
    let mut document: DocumentMut = generate_default().parse().unwrap();
    document["provider"]["anthropic"]["api_key"] = value(format!("file:{}", path.display()));
    fs::write(config, document.to_string()).unwrap();
}

#[tokio::test]
async fn existing_file_credentials_outside_policy_migrate_on_save() {
    for in_workspace in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let connection_options = options(directory.path(), None);
        let source_dir = if in_workspace {
            connection_options
                .workspace
                .as_ref()
                .unwrap()
                .join("old-credentials")
        } else {
            directory.path().join("other-data/credentials")
        };
        let source = private_key(&source_dir, b"fixture-only-not-a-real-key");
        let config = directory.path().join("existing.toml");
        config_with_file_key(&config, &source);
        let settings = Settings::load(options(directory.path(), Some(config.clone())))
            .await
            .unwrap();
        let state = SetupState::new(&settings);
        assert!(state.credential_ready());
        assert!(state
            .saved_key_hint(&settings)
            .unwrap()
            .contains("move it into this data directory"));

        save(&settings, &state).unwrap();

        let parsed = load_config(&config).unwrap();
        let Some(migrated) = parsed.provider["anthropic"].api_key.strip_prefix("file:") else {
            panic!("stored key source was not preserved");
        };
        let migrated = std::path::Path::new(migrated);
        assert!(migrated.starts_with(&settings.credentials_dir));
        assert!(migrated != source.as_path());
        assert!(source.exists());
        assert!(!fs::read_to_string(config)
            .unwrap()
            .contains("fixture-only-not-a-real-key"));
    }
}

#[tokio::test]
async fn safe_existing_file_reference_is_kept_without_creating_another_key() {
    let directory = tempfile::tempdir().unwrap();
    let connection_options = options(directory.path(), None);
    let credentials = connection_options
        .data_dir
        .as_ref()
        .unwrap()
        .join("credentials");
    let source = private_key(&credentials, b"fixture-only-not-a-real-key");
    let config = directory.path().join("existing.toml");
    config_with_file_key(&config, &source);
    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let state = SetupState::new(&settings);

    save(&settings, &state).unwrap();

    let parsed = load_config(&config).unwrap();
    assert!(parsed.provider["anthropic"].api_key == format!("file:{}", source.display()));
    assert_eq!(1, fs::read_dir(credentials).unwrap().count());
}

#[tokio::test]
async fn route_changes_clear_explicit_route_options_but_credential_edits_do_not() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("existing.toml");
    let mut document: DocumentMut = generate_default().parse().unwrap();
    document["model"]["balanced"]["reasoning_effort"] = value("low");
    document["model"]["balanced"]["service_tier"] = value("flex");
    fs::write(&config, document.to_string()).unwrap();

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    state.environment.set_text("NEW_API_KEY".into());
    save(&settings, &state).unwrap();
    let after_credentials: DocumentMut = fs::read_to_string(&config).unwrap().parse().unwrap();
    assert!(after_credentials["model"]["balanced"]
        .get("reasoning_effort")
        .is_some());
    assert!(after_credentials["model"]["balanced"]
        .get("service_tier")
        .is_some());

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    state.model.set_text("changed-route-model".into());
    save(&settings, &state).unwrap();
    let after_route: DocumentMut = fs::read_to_string(config).unwrap().parse().unwrap();
    assert!(after_route["model"]["balanced"]
        .get("reasoning_effort")
        .is_none());
    assert!(after_route["model"]["balanced"]
        .get("service_tier")
        .is_none());
}

#[tokio::test]
async fn stored_keys_are_referenced_by_path_not_written_to_config() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    fs::write(&config, generate_default()).unwrap();
    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());
    save(&settings, &state).unwrap();

    let output = fs::read_to_string(config).unwrap();
    assert!(!output.contains("fixture-only-not-a-real-key"));
    let parsed = load_config(&settings.config_path)
        .unwrap()
        .resolve()
        .unwrap();
    let ApiKey::File(path) = &parsed.providers["anthropic"].api_key else {
        panic!("expected an explicit file credential source");
    };
    assert!(path.starts_with(&settings.credentials_dir));
    assert_eq!(
        "fixture-only-not-a-real-key",
        fs::read_to_string(path).unwrap()
    );
}

#[tokio::test]
async fn concurrent_config_edit_removes_only_the_newly_staged_key() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    fs::write(&config, generate_default()).unwrap();
    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    let error = save_with_hook(&settings, &state, || {
        fs::write(&config, "external concurrent edit").unwrap();
    })
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("changed while settings were open"));
    assert_eq!(
        "external concurrent edit",
        fs::read_to_string(config).unwrap()
    );
    assert_eq!(0, fs::read_dir(&settings.credentials_dir).unwrap().count());
}

#[tokio::test]
async fn stored_keys_are_refused_when_data_directory_is_inside_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let options = ConnectionOptions {
        workspace: Some(workspace.clone()),
        data_dir: Some(workspace.join("data")),
        ..ConnectionOptions::default()
    };
    let settings = Settings::load(options).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    let error = save(&settings, &state).unwrap_err();
    assert!(error.to_string().contains("inside the workspace"));
    assert!(!settings.credentials_dir.exists());
    assert!(!settings.config_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn credentials_directory_symlink_is_rejected_without_following_it() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let data_dir = directory.path().join("data");
    let outside = directory.path().join("outside");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, data_dir.join("credentials")).unwrap();
    let options = ConnectionOptions {
        workspace: Some(workspace),
        data_dir: Some(data_dir),
        ..ConnectionOptions::default()
    };
    let settings = Settings::load(options).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    assert!(save(&settings, &state).is_err());
    assert_eq!(0, fs::read_dir(outside).unwrap().count());
}
