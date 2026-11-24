use std::fs;
use std::io::{self, Write};
use std::sync::{Arc, Barrier};

use kuku::config::{ApiKey, generate_default, load_config};
use kuku::util::private_directory;
use kuku_client::ConnectionOptions;
use toml_edit::{DocumentMut, value};

use super::{Settings, save, save_with_hook, save_with_lookup, stage_key_with_sync};
use crate::setup::state::SetupState;
use crate::setup::{CredentialSource, should_offer_setup};

const INLINE_CONFIG: &str = r#"
default_model = "balanced"
unrelated_root = "keep"
model = { strong = { provider = "anthropic", model = "strong-model", think = "high" }, balanced = { provider = "anthropic", model = "balanced-model", think = "medium", reasoning_effort = "low", service_tier = "flex" }, light = { provider = "anthropic", model = "light-model", think = "off" } }
provider = { anthropic = { format = "anthropic", base_url = "https://api.anthropic.com", api_key = "$ANTHROPIC_API_KEY" }, openai = { format = "openai-responses", base_url = "https://existing-openai.invalid/v1", api_key = "$OPENAI_API_KEY" } }
"#;

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

fn select_environment(state: &mut SetupState) {
    state.credential_source = CredentialSource::Environment;
}

#[tokio::test]
async fn missing_default_file_credential_reopens_with_current_route_and_preserves_config() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let missing_key = directory.path().join("missing.credential");
    let mut document: DocumentMut = INLINE_CONFIG.parse().unwrap();
    document["provider"]["anthropic"]["api_key"] = value(format!("file:{}", missing_key.display()));
    let original = document.to_string();
    fs::write(&config, &original).unwrap();

    let settings = Settings::load_for_first_run(options(directory.path(), Some(config.clone())))
        .await
        .unwrap()
        .unwrap();
    let state = SetupState::new(&settings);

    assert_eq!("balanced", settings.default_tier);
    assert!(!settings.default_route_credential_available());
    assert!(should_offer_setup(
        Ok(false),
        settings.existing,
        settings.default_route_credential_available()
    ));
    assert!(matches!(
        state.choice(),
        crate::setup::ConnectionChoice::Existing(name) if name == "anthropic"
    ));
    assert_eq!("balanced-model", state.model.text());
    assert_eq!(original, fs::read_to_string(config).unwrap());
}

async fn assert_first_run_uses_managed_data_dir(caller_home_name: Option<&str>) {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let missing_key = directory.path().join("missing.credential");
    let caller_home = caller_home_name.map(|name| directory.path().join(name));
    let mut document: DocumentMut = INLINE_CONFIG.parse().unwrap();
    document["provider"]["anthropic"]["api_key"] = value(format!("file:{}", missing_key.display()));
    let original = format!("{document}\n[discovery]\nextra_user_paths = [\"$KUKU_HOME\"]\n");
    fs::write(&config, original).unwrap();
    let environment = |name: &str| match name {
        "KUKU_HOME" => match &caller_home {
            Some(path) => Ok(path.to_string_lossy().into_owned()),
            None => Err(std::env::VarError::NotPresent),
        },
        _ => std::env::var(name),
    };

    let settings = Settings::load_for_first_run_with_lookup(
        options(directory.path(), Some(config.clone())),
        &environment,
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        vec![settings.data_dir.clone()],
        settings.raw.discovery.as_ref().unwrap().extra_user_paths
    );
    if let Some(caller_home) = &caller_home {
        assert_ne!(
            vec![caller_home.clone()],
            settings.raw.discovery.as_ref().unwrap().extra_user_paths
        );
    }
    assert!(!settings.default_route_credential_available());
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());
    save_with_lookup(&settings, &state, &environment, || {}).unwrap();
    assert!(
        fs::read_to_string(config)
            .unwrap()
            .contains("extra_user_paths = [\"$KUKU_HOME\"]")
    );
}

#[tokio::test]
async fn first_run_uses_managed_data_dir_for_kuku_home_references() {
    assert_first_run_uses_managed_data_dir(None).await;
}

#[tokio::test]
async fn first_run_overrides_set_caller_kuku_home_with_managed_data_dir() {
    assert_first_run_uses_managed_data_dir(Some("caller-home")).await;
}

#[tokio::test]
async fn default_route_credential_check_ignores_unrelated_provider_and_empty_values() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    fs::write(&config, INLINE_CONFIG).unwrap();
    let settings = Settings::load(options(directory.path(), Some(config)))
        .await
        .unwrap();
    let mut resolved = Vec::new();

    assert!(
        !settings.default_route_credential_available_with(|key| match key {
            ApiKey::Env(name) => {
                resolved.push(name.clone());
                None
            }
            _ => unreachable!(),
        })
    );
    assert_eq!(vec!["ANTHROPIC_API_KEY"], resolved);
    assert!(!settings.default_route_credential_available_with(|_| Some(String::new())));
    assert!(!settings.default_route_credential_available_with(|_| Some("  ".into())));
    assert!(should_offer_setup(
        Ok(false),
        true,
        settings.default_route_credential_available_with(|_| Some(String::new()))
    ));
    assert!(settings.default_route_credential_available_with(|_| {
        Some("fixture-only-not-a-real-key".into())
    }));
    assert!(should_offer_setup(Ok(false), true, false));
}

#[tokio::test]
async fn valid_plaintext_default_route_skips_repair_even_when_other_provider_key_is_missing() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let mut document: DocumentMut = INLINE_CONFIG.parse().unwrap();
    document["provider"]["anthropic"]["api_key"] = value("fixture-only-not-a-real-key");
    fs::write(&config, document.to_string()).unwrap();

    assert!(
        Settings::load_for_first_run(options(directory.path(), Some(config)))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn valid_private_file_default_route_skips_first_run_repair() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let options = options(directory.path(), Some(config.clone()));
    let settings = Settings::load(options.clone()).await.unwrap();
    let mut staged =
        stage_key_with_sync(&settings, "fixture-only-not-a-real-key", |_, _| Ok(())).unwrap();
    let key_path = staged.path().to_owned();
    staged.keep = true;

    let mut document: DocumentMut = generate_default().parse().unwrap();
    document["provider"]["anthropic"]["api_key"] = value(format!("file:{}", key_path.display()));
    fs::write(&config, document.to_string()).unwrap();

    assert!(
        Settings::load_for_first_run(options)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn credential_repair_keeps_unrelated_config_and_never_writes_key_text() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let missing_key = directory.path().join("missing.credential");
    let mut document: DocumentMut = INLINE_CONFIG.parse().unwrap();
    document["provider"]["anthropic"]["api_key"] = value(format!("file:{}", missing_key.display()));
    fs::write(&config, document.to_string()).unwrap();

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    let key = "fixture-only-not-a-real-key";
    state.credential_source = CredentialSource::StoredKey;
    state.stored_key.set_text(key.into());
    save(&settings, &state).unwrap();

    let output = fs::read_to_string(&config).unwrap();
    assert!(!output.contains(key));
    assert!(output.contains("unrelated_root = \"keep\""));
    assert!(output.contains("api_key = \"$OPENAI_API_KEY\""));
    assert!(output.contains("service_tier = \"flex\""));
    let parsed = load_config(&config).unwrap();
    let api_key = &parsed.provider["anthropic"].api_key;
    let path = std::path::Path::new(api_key.strip_prefix("file:").unwrap());
    assert!(path.starts_with(&settings.credentials_dir));
    assert!(kuku::util::is_valid_private_key(path));
}

#[tokio::test]
async fn invalid_existing_config_is_not_replaced_by_first_run_setup() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("invalid.toml");
    let original = b"[model\ninvalid";
    fs::write(&config, original).unwrap();

    assert!(
        Settings::load_for_first_run(options(directory.path(), Some(config.clone())))
            .await
            .is_err()
    );
    assert_eq!(original, fs::read(config).unwrap().as_slice());
}

#[test]
fn credential_source_defaults_are_deterministic_without_environment_mutation() {
    let environment = ApiKey::Env("FIXTURE_API_KEY".into());
    let file = ApiKey::File(std::path::PathBuf::from("fixture.credential"));
    let plaintext = ApiKey::Plaintext("fixture-only-not-a-real-key".into());
    assert_eq!(
        CredentialSource::StoredKey,
        super::preferred_source(Some(&environment), false, true)
    );
    assert_eq!(
        CredentialSource::Environment,
        super::preferred_source(Some(&environment), true, true)
    );
    assert_eq!(
        CredentialSource::StoredKey,
        super::preferred_source(None, false, true)
    );
    assert_eq!(
        CredentialSource::Environment,
        super::preferred_source(None, true, true)
    );
    assert_eq!(
        CredentialSource::Environment,
        super::preferred_source(Some(&environment), false, false)
    );
    assert_eq!(
        CredentialSource::StoredKey,
        super::preferred_source(Some(&plaintext), false, false)
    );
    assert_eq!(
        CredentialSource::StoredKey,
        super::preferred_source(Some(&file), false, false)
    );
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
    select_environment(&mut state);
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
async fn saving_uses_sdk_environment_expansion_and_preserves_references() {
    const TIER: &str = "_KUKU_SETUP_TEST_TIER";
    const PROVIDER: &str = "_KUKU_SETUP_TEST_PROVIDER";
    const MODEL: &str = "_KUKU_SETUP_TEST_MODEL";
    const FORMAT: &str = "_KUKU_SETUP_TEST_FORMAT";
    if !kuku::test_support::isolate_test(
        module_path!(),
        "saving_uses_sdk_environment_expansion_and_preserves_references",
        |command| {
            command
                .env(TIER, "balanced")
                .env(PROVIDER, "anthropic")
                .env(MODEL, "fixture-env-model")
                .env(FORMAT, "anthropic");
        },
    ) {
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let contents = generate_default()
        .replace(
            "default_model = \"balanced\"",
            &format!("default_model = \"${TIER}\""),
        )
        .replace(
            "provider = \"anthropic\"",
            &format!("provider = \"${PROVIDER}\""),
        )
        .replace(
            "model = \"claude-sonnet-4-6\"",
            &format!("model = \"${MODEL}\""),
        )
        .replace("format = \"anthropic\"", &format!("format = \"${FORMAT}\""));
    fs::write(&config, contents).unwrap();

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    select_environment(&mut state);
    save(&settings, &state).unwrap();

    let output = fs::read_to_string(&config).unwrap();
    assert!(output.contains(&format!("default_model = \"${TIER}\"")));
    assert!(output.contains(&format!("provider = \"${PROVIDER}\"")));
    assert!(output.contains(&format!("model = \"${MODEL}\"")));
    assert!(output.contains(&format!("format = \"${FORMAT}\"")));
    let parsed = load_config(&config).unwrap();
    assert_eq!(Some("balanced"), parsed.default_model.as_deref());
    assert_eq!("anthropic", parsed.model["strong"].provider);
    assert_eq!("fixture-env-model", parsed.model["strong"].model);
    assert_eq!("anthropic", parsed.provider["anthropic"].format.as_str());
}

#[tokio::test]
async fn prefill_uses_normalized_custom_default_route_and_preserves_options() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let mut document: DocumentMut = generate_default().parse().unwrap();
    let mut preferred = document["model"]["balanced"].clone();
    preferred["provider"] = value(" openai ");
    preferred["model"] = value(" route-model ");
    preferred["reasoning_effort"] = value("high");
    preferred["service_tier"] = value("priority");
    document["model"]["preferred"] = preferred;
    document["default_model"] = value("preferred");
    fs::write(&config, document.to_string()).unwrap();

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    select_environment(&mut state);
    assert_eq!("preferred", settings.default_tier);
    assert!(matches!(
        state.choice(),
        crate::setup::ConnectionChoice::Existing(name) if name == "openai"
    ));
    assert_eq!("route-model", state.model.text());
    save(&settings, &state).unwrap();

    let parsed = load_config(&config).unwrap();
    let preferred = &parsed.model["preferred"];
    assert_eq!("openai", preferred.provider);
    assert_eq!("route-model", preferred.model);
    assert_eq!(Some("high"), preferred.reasoning_effort.as_deref());
    assert_eq!(Some("priority"), preferred.service_tier.as_deref());
}

#[tokio::test]
async fn settings_edit_inline_model_and_provider_tables() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("inline.toml");
    fs::write(&config, INLINE_CONFIG).unwrap();
    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    select_environment(&mut state);
    state.environment.set_text("MIGRATED_API_KEY".into());
    save(&settings, &state).unwrap();

    let output = fs::read_to_string(&config).unwrap();
    assert!(output.contains("model = {"));
    assert!(output.contains("provider = {"));
    assert!(output.contains("unrelated_root = \"keep\""));
    let parsed = load_config(&config).unwrap();
    assert_eq!("$MIGRATED_API_KEY", parsed.provider["anthropic"].api_key);
    assert_eq!(
        "https://existing-openai.invalid/v1",
        parsed.provider["openai"].base_url
    );
    assert_eq!(
        Some("low"),
        parsed.model["balanced"].reasoning_effort.as_deref()
    );
    assert_eq!(
        Some("flex"),
        parsed.model["balanced"].service_tier.as_deref()
    );
}

#[tokio::test]
async fn preset_save_adds_an_inline_provider_without_overwriting_existing() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("inline.toml");
    fs::write(&config, INLINE_CONFIG).unwrap();
    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    select_environment(&mut state);
    let preset = crate::setup::ProviderPreset::OpenAi;
    state.selected = state
        .choices
        .iter()
        .position(|choice| *choice == crate::setup::ConnectionChoice::Preset(preset))
        .unwrap();
    state.model.set_text(preset.default_model().into());
    state.load_selection(&settings);
    select_environment(&mut state);
    state.environment.set_text("OPENAI_SETUP_KEY".into());
    save(&settings, &state).unwrap();

    let output = fs::read_to_string(&config).unwrap();
    assert!(output.contains("provider = {"));
    assert!(output.contains("openai_setup = {"));
    let parsed = load_config(&config).unwrap();
    assert_eq!(
        "https://existing-openai.invalid/v1",
        parsed.provider["openai"].base_url
    );
    assert_eq!("$OPENAI_SETUP_KEY", parsed.provider["openai_setup"].api_key);
    assert_eq!("openai_setup", parsed.model["balanced"].provider);
}

#[tokio::test]
async fn adding_a_duplicate_model_saves_a_short_route_name_and_keeps_its_connection() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("inline.toml");
    let mut document: DocumentMut = INLINE_CONFIG.parse().unwrap();
    let model = "gpt-6.1-terra";
    document["model"][model] = "{ provider = \"anthropic\", model = \"gpt-6.1-terra\" }"
        .parse::<toml_edit::Value>()
        .map(toml_edit::Item::Value)
        .unwrap();
    fs::write(&config, document.to_string()).unwrap();

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::for_intent(&settings, crate::setup::SetupIntent::Add);
    state.selected = state
        .choices
        .iter()
        .position(|choice| {
            matches!(choice, crate::setup::ConnectionChoice::Existing(name) if name == "openai")
        })
        .unwrap();
    state.load_selection(&settings);
    state.model.set_text(model.into());
    let resolved = save(&settings, &state).unwrap();

    let parsed = load_config(&config).unwrap();
    let short_route = "gpt-6.1-terra-2";
    assert_eq!("anthropic", parsed.model[model].provider);
    assert_eq!("openai", parsed.model[short_route].provider);
    assert_eq!(model, parsed.model[short_route].model);
    assert!(
        resolved
            .tier_infos()
            .iter()
            .any(|info| info.name == short_route)
    );
    assert!(!parsed.model.contains_key("openai/gpt-6.1-terra"));
}

#[tokio::test]
async fn new_configuration_saves_a_single_default_route() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path(), None))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    select_environment(&mut state);
    state.address.set_text("https://provider.invalid/v1".into());
    state.model.set_text("fixture-model".into());
    save(&settings, &state).unwrap();

    let parsed = load_config(&settings.config_path).unwrap();
    assert_eq!(Some("default"), parsed.default_model.as_deref());
    assert_eq!(1, parsed.model.len());
    assert_eq!("fixture-model", parsed.model["default"].model);
    assert_eq!("provider.invalid", parsed.model["default"].provider);
    assert_eq!(
        "https://provider.invalid/v1",
        parsed.provider["provider.invalid"].base_url
    );
    assert_eq!(
        Some("provider-default"),
        parsed.model["default"].think.as_deref()
    );
}

#[tokio::test]
async fn all_provider_presets_generate_parseable_configs() {
    for preset in crate::setup::ProviderPreset::ALL {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings::load(options(directory.path(), None))
            .await
            .unwrap();
        let mut state = SetupState::new(&settings);
        select_environment(&mut state);
        state.selected = state
            .choices
            .iter()
            .position(|choice| *choice == crate::setup::ConnectionChoice::Preset(preset))
            .unwrap();
        state.load_selection(&settings);
        select_environment(&mut state);
        state.model.set_text(preset.default_model().into());
        state.environment.set_text(preset.default_env().into());
        save(&settings, &state).unwrap();

        let parsed = load_config(&settings.config_path).unwrap();
        assert_eq!(preset.connection(), parsed.model["default"].provider);
        assert_eq!(preset.default_model(), parsed.model["default"].model);
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
    #[cfg(windows)]
    kuku::util::set_config_temp_security(None, file.path(), file.as_file()).unwrap();
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
        assert!(
            state
                .saved_key_hint(&settings)
                .unwrap()
                .contains("copy your saved key here")
        );

        save(&settings, &state).unwrap();

        let parsed = load_config(&config).unwrap();
        let Some(migrated) = parsed.provider["anthropic"].api_key.strip_prefix("file:") else {
            panic!("stored key source was not preserved");
        };
        let migrated = std::path::Path::new(migrated);
        assert!(migrated.starts_with(&settings.credentials_dir));
        assert!(migrated != source.as_path());
        assert!(source.exists());
        assert!(
            !fs::read_to_string(config)
                .unwrap()
                .contains("fixture-only-not-a-real-key")
        );
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
async fn route_changes_choose_new_defaults_but_credential_edits_preserve_settings() {
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
    select_environment(&mut state);
    state.environment.set_text("NEW_API_KEY".into());
    save(&settings, &state).unwrap();
    let after_credentials: DocumentMut = fs::read_to_string(&config).unwrap().parse().unwrap();
    assert!(
        after_credentials["model"]["balanced"]
            .get("reasoning_effort")
            .is_some()
    );
    assert!(
        after_credentials["model"]["balanced"]
            .get("service_tier")
            .is_some()
    );

    let settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut state = SetupState::new(&settings);
    select_environment(&mut state);
    state.model.set_text("changed-route-model".into());
    save(&settings, &state).unwrap();
    let after_route: DocumentMut = fs::read_to_string(config).unwrap().parse().unwrap();
    assert_eq!(
        after_route["model"]["balanced"]["reasoning_effort"].as_str(),
        Some("none")
    );
    assert!(
        after_route["model"]["balanced"]
            .get("service_tier")
            .is_none()
    );
}

#[tokio::test]
async fn credential_repair_preserves_unchanged_route_environment_references() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let missing_key = directory.path().join("missing.credential");
    let mut document: DocumentMut = INLINE_CONFIG.parse().unwrap();
    document["model"]["balanced"]["provider"] = value("$ROUTE_PROVIDER");
    document["model"]["balanced"]["model"] = value("$ROUTE_MODEL");
    document["provider"]["anthropic"]["api_key"] = value(format!("file:{}", missing_key.display()));
    let original = format!("{document}\n[discovery]\nextra_user_paths = [\"$KUKU_HOME\"]\n");
    fs::write(&config, original).unwrap();
    let environment = |name: &str| match name {
        "ROUTE_PROVIDER" => Ok("anthropic".into()),
        "ROUTE_MODEL" => Ok("model-from-environment".into()),
        "KUKU_HOME" => Err(std::env::VarError::NotPresent),
        _ => std::env::var(name),
    };
    let settings = Settings::load_for_first_run_with_lookup(
        options(directory.path(), Some(config.clone())),
        &environment,
    )
    .await
    .unwrap()
    .unwrap();
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    save_with_lookup(&settings, &state, &environment, || {}).unwrap();
    let credential_only: DocumentMut = fs::read_to_string(&config).unwrap().parse().unwrap();
    assert_eq!(
        "$ROUTE_PROVIDER",
        credential_only["model"]["balanced"]["provider"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        "$ROUTE_MODEL",
        credential_only["model"]["balanced"]["model"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        "low",
        credential_only["model"]["balanced"]["reasoning_effort"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        "flex",
        credential_only["model"]["balanced"]["service_tier"]
            .as_str()
            .unwrap()
    );

    let settings = Settings::load_with_lookup(
        options(directory.path(), Some(config.clone())),
        &environment,
    )
    .await
    .unwrap();
    let mut state = SetupState::new(&settings);
    state.model.set_text("intentional-new-model".into());
    save_with_lookup(&settings, &state, &environment, || {}).unwrap();
    let route_changed: DocumentMut = fs::read_to_string(config).unwrap().parse().unwrap();
    assert_eq!(
        "anthropic",
        route_changed["model"]["balanced"]["provider"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        "intentional-new-model",
        route_changed["model"]["balanced"]["model"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        route_changed["model"]["balanced"]["reasoning_effort"].as_str(),
        Some("none")
    );
    assert!(
        route_changed["model"]["balanced"]
            .get("service_tier")
            .is_none()
    );
}

#[tokio::test]
async fn intentional_route_change_pins_selected_route_across_environment_divergence() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    let missing_key = directory.path().join("missing.credential");
    let mut document: DocumentMut = INLINE_CONFIG.parse().unwrap();
    document["model"]["balanced"]["provider"] = value("$ROUTE_PROVIDER");
    document["model"]["balanced"]["model"] = value("$ROUTE_MODEL");
    document["provider"]["anthropic"]["api_key"] = value(format!("file:{}", missing_key.display()));
    let original = format!("{document}\n[discovery]\nextra_user_paths = [\"$KUKU_HOME\"]\n");
    fs::write(&config, original).unwrap();
    let caller_environment = |name: &str| match name {
        "ROUTE_PROVIDER" => Ok("anthropic".into()),
        "ROUTE_MODEL" => Ok("caller-model".into()),
        _ => Err(std::env::VarError::NotPresent),
    };
    let service_environment = |name: &str| match name {
        "ROUTE_PROVIDER" => Ok("openai".into()),
        "ROUTE_MODEL" => Ok("service-model".into()),
        _ => Err(std::env::VarError::NotPresent),
    };
    let settings = Settings::load_for_first_run_with_lookup(
        options(directory.path(), Some(config.clone())),
        &caller_environment,
    )
    .await
    .unwrap()
    .unwrap();
    let mut state = SetupState::new(&settings);
    state.model.set_text("selected-model".into());
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    save_with_lookup(&settings, &state, &caller_environment, || {}).unwrap();

    let written: DocumentMut = fs::read_to_string(&config).unwrap().parse().unwrap();
    assert_eq!(
        "anthropic",
        written["model"]["balanced"]["provider"].as_str().unwrap()
    );
    assert_eq!(
        "selected-model",
        written["model"]["balanced"]["model"].as_str().unwrap()
    );

    let service_settings = Settings::load_with_lookup(
        options(directory.path(), Some(config)),
        &service_environment,
    )
    .await
    .unwrap();
    let service_route = service_settings.resolved.tiers.get("balanced").unwrap();
    assert_eq!("anthropic", service_route.provider);
    assert_eq!("selected-model", service_route.model);
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
    assert!(
        error
            .to_string()
            .contains("changed while settings were open")
    );
    assert_eq!(
        "external concurrent edit",
        fs::read_to_string(config).unwrap()
    );
    assert_eq!(0, fs::read_dir(&settings.credentials_dir).unwrap().count());
}

#[tokio::test]
async fn simultaneous_settings_saves_allow_only_one_writer() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("custom.toml");
    fs::write(&config, generate_default()).unwrap();
    let first_settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let second_settings = Settings::load(options(directory.path(), Some(config.clone())))
        .await
        .unwrap();
    let mut first_state = SetupState::new(&first_settings);
    select_environment(&mut first_state);
    first_state.model.set_text("writer-one-model".into());
    let mut second_state = SetupState::new(&second_settings);
    select_environment(&mut second_state);
    second_state.model.set_text("writer-two-model".into());
    let barrier = Arc::new(Barrier::new(2));

    let first_barrier = Arc::clone(&barrier);
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        save(&first_settings, &first_state)
    });
    let second_barrier = Arc::clone(&barrier);
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        save(&second_settings, &second_state)
    });
    let first = first.join().unwrap();
    let second = second.join().unwrap();

    assert!(first.is_ok() != second.is_ok());
    let saved = load_config(&config).unwrap();
    assert!(
        saved.model["balanced"].model == "writer-one-model"
            || saved.model["balanced"].model == "writer-two-model"
    );
}

#[tokio::test]
async fn staged_key_is_removed_when_publication_sync_fails() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path(), None))
        .await
        .unwrap();

    let error = stage_key_with_sync(
        &settings,
        "fixture-only-not-a-real-key",
        |path, directory_created| {
            assert_eq!(path.parent(), Some(settings.credentials_dir.as_path()));
            assert!(path.is_file());
            assert!(
                kuku::util::is_valid_private_key(path),
                "staged credentials must be private before publication sync"
            );
            assert!(directory_created);
            Err(io::Error::other("fixture sync failure"))
        },
    )
    .err()
    .unwrap();

    assert!(
        error
            .to_string()
            .contains("cannot sync private credential storage")
    );
    assert!(!error.to_string().contains("fixture-only-not-a-real-key"));
    assert_eq!(0, fs::read_dir(&settings.credentials_dir).unwrap().count());
}

#[tokio::test]
async fn stored_keys_are_private_when_data_directory_is_inside_workspace() {
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
    state.address.set_text("https://provider.invalid/v1".into());
    state.model.set_text("fixture-model".into());
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    assert!(settings.can_store_key());
    let config = save(&settings, &state).unwrap();
    let provider = &config.providers[&config.tiers[&config.default_tier].provider];
    let ApiKey::File(path) = &provider.api_key else {
        panic!("expected a private credential reference");
    };
    assert_eq!(path.parent(), Some(settings.credentials_dir.as_path()));
    assert!(kuku::util::is_valid_private_key(path));
    assert_eq!(
        "fixture-only-not-a-real-key",
        fs::read_to_string(path).unwrap()
    );
    assert!(
        !fs::read_to_string(&settings.config_path)
            .unwrap()
            .contains("fixture-only-not-a-real-key")
    );
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
