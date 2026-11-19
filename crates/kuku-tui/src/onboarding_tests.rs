use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use kuku::config::{load_config, ApiKey, ProviderProfile};
use kuku_client::ConnectionOptions;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tempfile::TempDir;

use super::{
    build_config, env_value_is_nonempty, missing_local_config, persist_setup, render,
    valid_env_name, valid_model_id, write_config_exclusive, Finish, ProviderPreset, SetupState,
    SetupStep,
};

fn options(data_dir: &std::path::Path) -> ConnectionOptions {
    ConnectionOptions {
        data_dir: Some(data_dir.to_owned()),
        ..ConnectionOptions::default()
    }
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn screen(state: &mut SetupState, width: u16, height: u16, credential_set: bool) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            render(
                frame,
                PathBuf::from("/work/project").as_path(),
                PathBuf::from("/home/user/.kuku/config.toml").as_path(),
                state,
                credential_set,
            )
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<Vec<_>>()
        .chunks(usize::from(width))
        .map(|row| row.concat())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn setup_is_offered_only_for_a_missing_local_config_and_resolves_selected_paths() {
    let directory = TempDir::new().unwrap();
    let default_options = options(directory.path());
    assert_eq!(
        Some(directory.path().join("config.toml")),
        missing_local_config(&default_options)
    );
    let config = directory.path().join("custom").join("agent.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let mut selected = options(directory.path());
    selected.config = Some(config.clone());
    assert_eq!(Some(config.clone()), missing_local_config(&selected));

    fs::write(&config, "not valid TOML = [").unwrap();
    assert_eq!(None, missing_local_config(&selected));

    let mut remote = selected.clone();
    remote.remote = Some("https://example.invalid".into());
    assert_eq!(None, missing_local_config(&remote));

    let mut token_only = selected.clone();
    token_only.token = Some("ignored-invalid-token".into());
    assert_eq!(None, missing_local_config(&token_only));
}

#[test]
fn existing_directories_and_dangling_symlinks_are_not_setup_targets() {
    let directory = TempDir::new().unwrap();
    let mut selected = options(directory.path());
    let config_dir = directory.path().join("config.toml");
    fs::create_dir(&config_dir).unwrap();
    assert_eq!(None, missing_local_config(&selected));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        fs::remove_dir(&config_dir).unwrap();
        symlink(directory.path().join("missing-target"), &config_dir).unwrap();
        assert_eq!(None, missing_local_config(&selected));
    }
    selected.config = Some(directory.path().join("explicit.toml"));
    assert!(missing_local_config(&selected).is_some());
}

#[test]
fn generated_provider_configs_parse_and_resolve_with_the_sdk_loader() {
    let directory = TempDir::new().unwrap();
    for provider in ProviderPreset::ALL {
        let model = provider.default_model();
        let contents = build_config(provider, model, provider.default_env()).unwrap();
        let path = directory
            .path()
            .join(format!("{}.toml", provider.connection()));
        write_config_exclusive(&path, &contents).unwrap();
        let config_file = load_config(&path).unwrap();
        let config = config_file.resolve().unwrap();
        assert_eq!("balanced", config.default_tier);
        assert_eq!(provider.connection(), config.tiers["balanced"].provider);
        assert_eq!(model, config.tiers["balanced"].model);
        for tier in ["strong", "balanced", "light"] {
            assert_eq!(provider.connection(), config.tiers[tier].provider);
            assert_eq!(model, config.tiers[tier].model);
        }
        assert!(matches!(
            &config.providers[provider.connection()].api_key,
            ApiKey::Env(name) if name == provider.default_env()
        ));
    }
}

#[test]
fn generated_config_escapes_edited_values_and_preserves_them_through_sdk_parsing() {
    let directory = TempDir::new().unwrap();
    let model = "organization/model-\"custom";
    let contents = build_config(ProviderPreset::OpenRouter, model, "MY_ROUTER_KEY").unwrap();
    let path = directory.path().join("config.toml");
    fs::write(path, contents).unwrap();
    let config = load_config(&directory.path().join("config.toml")).unwrap();
    assert_eq!(Some("balanced"), config.default_model.as_deref());
    assert_eq!(model, config.model["balanced"].model);
    assert_eq!(
        ProviderProfile::OpenRouter,
        config.provider["openrouter"].options.profile
    );
    assert_eq!("$MY_ROUTER_KEY", config.provider["openrouter"].api_key);
}

#[test]
fn model_and_environment_names_are_trimmed_and_validated() {
    assert_eq!(
        "claude-sonnet-4-6",
        valid_model_id("  claude-sonnet-4-6  ").unwrap()
    );
    assert!(valid_model_id(" \t ").is_err());
    assert!(valid_model_id("model\nname").is_err());
    assert!(valid_model_id("$MODEL_NAME").is_err());

    assert_eq!("API_KEY_1", valid_env_name(" API_KEY_1 ").unwrap());
    for name in ["", "1API_KEY", "API-KEY", "API KEY", "密钥"] {
        assert!(
            valid_env_name(name).is_err(),
            "unexpectedly accepted {name:?}"
        );
    }
}

#[test]
fn an_empty_credential_environment_variable_counts_as_missing() {
    assert!(!env_value_is_nonempty(None));
    assert!(!env_value_is_nonempty(Some(OsStr::new(""))));
    assert!(env_value_is_nonempty(Some(OsStr::new("configured"))));
}

#[test]
fn setup_transitions_to_save_and_exit_when_the_selected_environment_variable_is_missing() {
    let directory = TempDir::new().unwrap();
    let config_path = directory.path().join("config.toml");
    let mut state = SetupState::default();
    assert_eq!(SetupStep::Provider, state.step);
    state.handle(key(KeyCode::Down, KeyModifiers::NONE), false);
    assert_eq!(ProviderPreset::OpenAi, state.provider);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), false);
    assert_eq!(SetupStep::Model, state.step);
    state.handle(key(KeyCode::Char('a'), KeyModifiers::CONTROL), false);
    state.handle(Event::Paste("provider/model-id".into()), false);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), false);
    assert_eq!(SetupStep::Credential, state.step);
    state.handle(key(KeyCode::Char('a'), KeyModifiers::CONTROL), false);
    state.handle(Event::Paste("MISSING_PROVIDER_KEY".into()), false);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), false);
    assert_eq!(SetupStep::Review, state.step);
    assert_eq!("provider/model-id", state.model.text());
    assert_eq!("MISSING_PROVIDER_KEY", state.credential.text());
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), false);
    assert_eq!(Some(Finish::SaveAndExit), state.finish);
    persist_setup(&config_path, &state, state.finish.unwrap()).unwrap();
    let config = load_config(&config_path).unwrap().resolve().unwrap();
    assert_eq!("openai", config.tiers["balanced"].provider);
    assert_eq!("provider/model-id", config.tiers["balanced"].model);
    assert!(matches!(
        &config.providers["openai"].api_key,
        ApiKey::Env(name) if name == "MISSING_PROVIDER_KEY"
    ));
}

#[test]
fn setup_can_save_and_continue_when_variable_is_set_and_escape_returns_back() {
    let mut state = SetupState::default();
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    assert_eq!(SetupStep::Review, state.step);
    state.handle(key(KeyCode::Esc, KeyModifiers::NONE), true);
    assert_eq!(SetupStep::Credential, state.step);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    assert_eq!(Some(Finish::SaveAndContinue), state.finish);
}

#[test]
fn cancellation_has_no_save_action_and_does_not_create_configuration() {
    let directory = TempDir::new().unwrap();
    let config = directory.path().join("config.toml");
    let mut state = SetupState::default();
    state.handle(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false);
    assert_eq!(Some(Finish::Cancel), state.finish);
    persist_setup(&config, &state, state.finish.unwrap()).unwrap();
    assert!(!config.exists());
}

#[test]
fn exclusive_save_never_overwrites_an_existing_configuration() {
    let directory = TempDir::new().unwrap();
    let config = directory.path().join("config.toml");
    fs::write(&config, "existing config").unwrap();
    let error = write_config_exclusive(&config, "new config").unwrap_err();
    assert_eq!(std::io::ErrorKind::AlreadyExists, error.kind());
    assert_eq!("existing config", fs::read_to_string(config).unwrap());
}

#[test]
fn missing_environment_and_narrow_screens_render_without_showing_secret_values() {
    let mut state = SetupState::default();
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), false);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), false);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), false);
    let wide = screen(&mut state, 100, 24, false);
    assert!(wide.contains("/work/project"));
    assert!(wide.contains("/home/user/.kuku/config.toml"));
    assert!(wide.contains("not set"));
    assert!(wide.contains("ANTHROPIC_API_KEY"));
    assert!(wide.contains("No model request is made by setup."));

    let narrow = screen(&mut state, 24, 7, false);
    assert!(narrow.contains("Terminal too small."));
    assert!(narrow.contains("Minimum: 60 columns"));
    assert!(narrow.contains("by 20 rows."));
    assert!(!wide.contains("actual-secret"));

    state.credential.set_text("SOME_PROVIDER_KEY".into());
    let set = screen(&mut state, 100, 24, true);
    assert!(set.contains("SOME_PROVIDER_KEY is set"));
    assert!(set.contains("credential was not validated"));
}

#[test]
fn undersized_setup_prompts_for_resize_and_only_allows_cancellation() {
    let mut state = SetupState::default();
    let rendered = screen(&mut state, 24, 7, false);
    assert!(rendered.contains("Terminal too small."));
    assert!(rendered.contains("Minimum: 60 columns"));
    assert!(rendered.contains("by 20 rows."));
    assert!(rendered.contains("Esc / Ctrl+C cancels."));
    assert!(!rendered.contains("OpenRouter"));
    assert!(!rendered.contains("Save and"));

    screen(&mut state, 1, 1, false);

    state.handle_with_size(key(KeyCode::Down, KeyModifiers::NONE), false, false);
    state.handle_with_size(key(KeyCode::Enter, KeyModifiers::NONE), false, false);
    state.handle_with_size(Event::Paste("unexpected-model".into()), false, false);
    assert_eq!(ProviderPreset::Anthropic, state.provider);
    assert_eq!(SetupStep::Provider, state.step);
    assert_eq!("claude-sonnet-4-6", state.model.text());

    state.handle_with_size(key(KeyCode::Esc, KeyModifiers::NONE), false, false);
    assert_eq!(Some(Finish::Cancel), state.finish);

    let mut state = SetupState::default();
    state.handle_with_size(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false, false);
    assert_eq!(Some(Finish::Cancel), state.finish);

    let mut state = SetupState::default();
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle_with_size(key(KeyCode::Enter, KeyModifiers::NONE), true, false);
    assert_eq!(SetupStep::Review, state.step);
    assert_eq!(None, state.finish);
}

#[test]
fn minimum_supported_terminal_fits_provider_and_review_choices() {
    let mut state = SetupState::default();
    let provider = screen(&mut state, 60, 20, false);
    for choice in ["Anthropic", "OpenAI", "DeepSeek official", "OpenRouter"] {
        assert!(provider.contains(choice), "missing provider {choice}");
    }

    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    state.handle(key(KeyCode::Enter, KeyModifiers::NONE), true);
    let review = screen(&mut state, 60, 20, true);
    for choice in ["Save and continue", "Save and exit", "Back", "Cancel"] {
        assert!(review.contains(choice), "missing review action {choice}");
    }
}
