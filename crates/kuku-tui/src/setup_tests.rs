use std::fs;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use kuku_client::ConnectionOptions;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use super::config::Settings;
use super::state::{ReviewAction, SetupState, SetupStep};
use super::{CredentialSource, Finish, SetupMode};

fn options(root: &std::path::Path) -> ConnectionOptions {
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    ConnectionOptions {
        workspace: Some(workspace),
        data_dir: Some(root.join("data")),
        ..ConnectionOptions::default()
    }
}

fn screen(
    settings: &Settings,
    state: &mut SetupState,
    mode: SetupMode,
    width: u16,
    height: u16,
) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| super::ui::render(frame, settings, state, mode))
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
fn init_gate_requires_a_terminal_and_local_options_without_creating_paths() {
    let directory = tempfile::tempdir().unwrap();
    let options = options(directory.path());
    let data_dir = options.data_dir.clone().unwrap();
    let error =
        super::validate_entry(&options, false, "Use a terminal or edit config.toml.").unwrap_err();
    assert!(error.to_string().contains("No files were changed"));
    assert!(!data_dir.exists());

    let mut remote = options.clone();
    remote.remote = Some("https://service.invalid".into());
    assert!(super::validate_entry(&remote, true, "guidance")
        .unwrap_err()
        .to_string()
        .contains("local-only"));
    assert!(!data_dir.exists());
}

#[tokio::test]
async fn stored_key_paste_is_masked_and_small_terminals_are_safe() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Credential;
    state.credential_source = CredentialSource::StoredKey;
    let fixture = "fixture-only-not-a-real-key";
    state.handle(Event::Paste(fixture.into()), &settings, SetupMode::Init);

    let rendered = screen(&settings, &mut state, SetupMode::Init, 80, 24);
    assert!(!rendered.contains(fixture));
    assert!(rendered.contains("fixture"));
    assert!(rendered.contains("•l-key"));
    assert_eq!("•••••••", super::ui::mask_key("1234567"));
    assert_eq!("12••••78", super::ui::mask_key("12345678"));

    let small = screen(&settings, &mut state, SetupMode::Init, 24, 7);
    assert!(small.contains("Terminal too small"));
    assert!(small.contains("Esc / Ctrl+C cancels"));
    state.step = SetupStep::Credential;
    assert!(!super::ui::handle_small_terminal_event(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ));
    assert!(super::ui::handle_small_terminal_event(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ));
    assert_eq!(Some(Finish::Cancel), state.finish);
    assert_eq!(0, fs::read_dir(&settings.data_dir).unwrap().count());
}

#[tokio::test]
async fn source_selection_backtracking_and_cancel_do_not_write_config_or_keys() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::Environment;
    state.step = SetupStep::CredentialSource;
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(CredentialSource::StoredKey, state.credential_source);
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(SetupStep::Network, state.step);
    state.step = SetupStep::Consent;
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(Some(Finish::Cancel), state.finish);
    assert!(!settings.config_path.exists());
    assert!(!settings.credentials_dir.exists());
}

#[tokio::test]
async fn minimum_size_renders_the_working_connection_choices() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Connection;
    let rendered = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(rendered.contains("Choose a provider"));
    assert!(rendered.contains("› New custom connection"));
    assert!(rendered.find("New custom connection") < rendered.find("Preset"));
    assert!(rendered
        .lines()
        .nth(18)
        .unwrap()
        .starts_with("│Esc previous step"));
    assert!(rendered.contains("Preset · Anthropic"));
    assert!(rendered.contains("Preset · OpenAI"));

    state.step = SetupStep::Consent;
    let consent = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(consent.lines().nth(18).unwrap().starts_with("│Esc exit"));
}

#[tokio::test]
async fn minimum_size_keeps_save_and_settings_separate() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Review;
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    state.model.set_text("fixture".into());
    state.address.set_text("https://fixture.test/v1".into());
    let init = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(init.contains("Save and exit"));
    assert!(init.contains("API URL"));
    assert!(init.contains("Settings"));
    assert!(!init.contains("Model capabilities"));

    let first_run = screen(&settings, &mut state, SetupMode::FirstRun, 60, 20);
    assert!(first_run.contains("› Save and continue"));
    assert!(!first_run.contains("Save and exit"));
    assert!(!first_run.contains("Configured USD rates"));

    state.step = SetupStep::ReviewSettings;
    let settings_page = screen(&settings, &mut state, SetupMode::FirstRun, 60, 20);
    assert!(settings_page.contains("Thinking"));
    assert!(settings_page.contains("Costs"));
    assert!(settings_page.contains("Model capabilities"));
    assert!(settings_page.contains("Save and exit"));
}

#[tokio::test]
async fn credential_source_status_is_visible_at_minimum_size() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::CredentialSource;
    state.credential_source = CredentialSource::Environment;
    state.environment.set_text("ANTHROPIC_API_KEY".into());
    state.environment_detected = false;

    let missing = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(missing.contains("Environment · not detected · ANTHROPIC_API_KEY"));
    assert!(missing.contains("Paste API key (save locally)"));
    assert!(missing.contains("Keys are tested after you choose a model."));

    state.environment_detected = true;
    let detected = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(detected.contains("Environment · detected · ANTHROPIC_API_KEY"));

    state.environment.set_text("K".repeat(200));
    state.environment_detected = false;
    let long_name = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(long_name.contains("Environment · not detected ·"));
}

#[tokio::test]
async fn environment_detection_refreshes_after_edits_paste_and_back_navigation() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.credential_source = CredentialSource::Environment;
    state.step = SetupStep::Credential;
    state
        .environment
        .set_text("KUKU_SETUP_PASTE_FIXTURE".into());
    let pasted_name = "KUKU_SETUP_PASTE_FIXTUREX";
    let pasted_expected = std::env::var(pasted_name).is_ok_and(|value| !value.trim().is_empty());
    state.handle(Event::Paste("X".into()), &settings, SetupMode::FirstRun);
    assert_eq!(pasted_expected, state.environment_set());

    state.environment.set_text("KUKU_SETUP_BACKSPACE_X".into());
    let edited_name = "KUKU_SETUP_BACKSPACE_";
    let edited_expected = std::env::var(edited_name).is_ok_and(|value| !value.trim().is_empty());
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(edited_expected, state.environment_set());

    state.environment.set_text("KUKU_SETUP_BACK_FIXTURE".into());
    let back_expected =
        std::env::var("KUKU_SETUP_BACK_FIXTURE").is_ok_and(|value| !value.trim().is_empty());
    state.environment_detected = !back_expected;
    state.step = SetupStep::Review;
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(SetupStep::ModelSource, state.step);
    assert_eq!(back_expected, state.environment_set());
}

#[tokio::test]
async fn first_run_missing_credential_defaults_to_paste_repair() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Review;
    state.credential_source = CredentialSource::Environment;
    state.environment.set_text("ANTHROPIC_API_KEY".into());
    state.environment_detected = false;

    assert_eq!(
        ReviewAction::RepairCredential,
        state.review_choices(SetupMode::FirstRun)[0]
    );
    assert_eq!(
        ReviewAction::SaveAndExit,
        state.review_choices(SetupMode::FirstRun)[1]
    );
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(SetupStep::Credential, state.step);
    assert_eq!(CredentialSource::StoredKey, state.credential_source);
    assert_eq!(None, state.finish);

    state.step = SetupStep::Review;
    state.credential_source = CredentialSource::Environment;
    state.environment_detected = false;
    state.review_selection = 1;
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(Some(Finish::SaveAndExit), state.finish);
}

#[tokio::test]
async fn explicit_environment_choice_is_not_automatically_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::CredentialSource;
    state.credential_source = CredentialSource::StoredKey;
    state.environment.set_text("MISSING_FIXTURE_KEY".into());
    state.environment_detected = false;
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(CredentialSource::Environment, state.credential_source);

    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(SetupStep::Credential, state.step);
    assert_eq!(CredentialSource::Environment, state.credential_source);
}

#[tokio::test]
async fn blank_paste_stays_on_credential_input_and_ready_sources_continue() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Credential;
    state.credential_source = CredentialSource::StoredKey;
    state.handle(Event::Paste(String::new()), &settings, SetupMode::FirstRun);
    state.handle(
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &settings,
        SetupMode::FirstRun,
    );
    assert_eq!(SetupStep::Credential, state.step);
    assert_eq!(None, state.finish);

    state.step = SetupStep::Review;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());
    state.credential_source = CredentialSource::StoredKey;
    assert_eq!(
        ReviewAction::SaveAndContinue,
        state.review_choices(SetupMode::FirstRun)[0]
    );
    state.stored_key.set_text(String::new());
    state.credential_source = CredentialSource::Environment;
    state.environment_detected = true;
    assert_eq!(
        ReviewAction::SaveAndContinue,
        state.review_choices(SetupMode::FirstRun)[0]
    );
}

#[tokio::test]
async fn unsafe_storage_shows_environment_guidance_and_never_defaults_to_save() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let data = workspace.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("credentials"), "").unwrap();
    let options = ConnectionOptions {
        workspace: Some(workspace),
        data_dir: Some(data),
        ..ConnectionOptions::default()
    };
    let settings = Settings::load(options).await.unwrap();
    assert!(!settings.can_store_key());
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Review;
    state.credential_source = CredentialSource::Environment;
    state.environment_detected = false;

    assert_eq!(
        ReviewAction::Back,
        state.review_choices(SetupMode::FirstRun)[0]
    );
    assert_eq!(
        ReviewAction::SaveAndExit,
        state.review_choices(SetupMode::FirstRun)[1]
    );
    let rendered = screen(&settings, &mut state, SetupMode::FirstRun, 60, 20);
    assert!(rendered.contains("Set the API key variable before starting."));
    assert!(rendered.contains("Save and exit"));
}

#[tokio::test]
async fn review_settings_return_to_save_and_keep_the_secondary_exit() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Review;
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());
    let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
    let mode = SetupMode::FirstRun;

    state.handle(key(KeyCode::Down), &settings, mode);
    state.handle(key(KeyCode::Enter), &settings, mode);
    assert_eq!(SetupStep::ReviewSettings, state.step);

    state.handle(key(KeyCode::Enter), &settings, mode);
    assert_eq!(SetupStep::Thinking, state.step);
    state.handle(key(KeyCode::Enter), &settings, mode);
    assert_eq!(SetupStep::ReviewSettings, state.step);
    state.handle(key(KeyCode::Esc), &settings, mode);
    assert_eq!(SetupStep::Review, state.step);
    assert_eq!(0, state.review_selection);

    state.handle(key(KeyCode::Down), &settings, mode);
    state.handle(key(KeyCode::Enter), &settings, mode);
    state.handle(key(KeyCode::Up), &settings, mode);
    state.handle(key(KeyCode::Enter), &settings, mode);
    assert_eq!(Some(Finish::SaveAndExit), state.finish);
}

#[tokio::test]
async fn discovery_failure_preserves_the_form_and_search_preserves_selection() {
    use kuku::adapter::DiscoveredModel;

    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
    state.step = SetupStep::ModelSource;
    state
        .address
        .set_text("https://fixture.example/gateway/infer".into());
    state.full_url = true;
    state.models_url = "https://fixture.example/catalog/models".into();
    state.credential_source = CredentialSource::StoredKey;
    state.stored_key.set_text("fixture-key".into());
    state.handle(key(KeyCode::Enter), &settings, SetupMode::FirstRun);
    assert!(state.discovery.rows().is_empty());
    assert_eq!("fixture-key", state.stored_key.text());
    assert_eq!(
        "https://fixture.example/gateway/infer",
        state.address.text()
    );

    state.models.found = ["alpha", "beta"]
        .into_iter()
        .map(|id| DiscoveredModel {
            id: id.into(),
            name: None,
            capabilities: Default::default(),
        })
        .collect();
    state.step = SetupStep::Discover;
    for query in ["alpha", "beta"] {
        state.models.query.set_text(query.into());
        state.handle(key(KeyCode::Char(' ')), &settings, SetupMode::FirstRun);
    }
    state.handle(key(KeyCode::Enter), &settings, SetupMode::FirstRun);
    assert_eq!(SetupStep::DefaultModel, state.step);
    state.handle(key(KeyCode::Down), &settings, SetupMode::FirstRun);
    state.handle(key(KeyCode::Enter), &settings, SetupMode::FirstRun);
    assert_eq!(Some("beta"), state.primary_model().as_deref());
    assert_eq!(vec!["alpha", "beta"], state.model_ids().unwrap());
    let config = super::config::save(&settings, &state).unwrap();
    assert_eq!("beta", config.tier(config.default_tier()).unwrap().model);
    assert_eq!(2, config.tiers.len());
}
