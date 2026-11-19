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
    assert!(rendered.contains(&"•".repeat(fixture.chars().count())));

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
    assert_eq!(SetupStep::Model, state.step);
    state.step = SetupStep::Connection;
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
    let rendered = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(rendered.contains("Step 1/5"));
    assert!(rendered.contains("Preset · Anthropic"));
    assert!(rendered.contains("Preset · OpenAI"));
}

#[tokio::test]
async fn minimum_size_shows_review_actions_in_both_modes() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Review;
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    let init = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(!init.contains("Save and continue"));
    assert!(init.contains("Save and exit"));
    assert!(init.contains("Back"));
    assert!(init.contains("Cancel"));

    let first_run = screen(&settings, &mut state, SetupMode::FirstRun, 60, 20);
    assert!(first_run.contains("Save and continue"));
    assert!(first_run.contains("Save and exit"));
    assert!(first_run.contains("Back"));
    assert!(first_run.contains("Cancel"));
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
    assert!(missing.contains("Environment · ANTHROPIC_API_KEY · not detected"));
    assert!(missing.contains("Paste API key (save locally)"));
    assert!(missing.contains("Presence only; credential not authenticated."));

    state.environment_detected = true;
    let detected = screen(&settings, &mut state, SetupMode::Init, 60, 20);
    assert!(detected.contains("Environment · ANTHROPIC_API_KEY · detected"));
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
    fs::create_dir_all(&workspace).unwrap();
    let options = ConnectionOptions {
        workspace: Some(workspace.clone()),
        data_dir: Some(workspace.join("data")),
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
    assert!(rendered.contains("Storage unavailable; set the variable before launch."));
    assert!(rendered.contains("Save and exit"));
}

#[tokio::test]
async fn review_keyboard_choices_dispatch_each_action() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings::load(options(directory.path())).await.unwrap();
    let mut state = SetupState::new(&settings);
    state.step = SetupStep::Review;
    state.credential_source = CredentialSource::StoredKey;
    state
        .stored_key
        .set_text("fixture-only-not-a-real-key".into());

    for (index, action) in [
        ReviewAction::SaveAndContinue,
        ReviewAction::SaveAndExit,
        ReviewAction::Back,
        ReviewAction::Cancel,
    ]
    .into_iter()
    .enumerate()
    {
        state.step = SetupStep::Review;
        state.review_selection = index;
        state.finish = None;
        state.handle(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &settings,
            SetupMode::FirstRun,
        );
        match action {
            ReviewAction::SaveAndContinue => {
                assert_eq!(Some(Finish::SaveAndContinue), state.finish)
            }
            ReviewAction::SaveAndExit => assert_eq!(Some(Finish::SaveAndExit), state.finish),
            ReviewAction::RepairCredential => unreachable!(),
            ReviewAction::Back => assert_eq!(SetupStep::Credential, state.step),
            ReviewAction::Cancel => assert_eq!(Some(Finish::Cancel), state.finish),
        }
    }

    for (index, action) in [
        ReviewAction::SaveAndExit,
        ReviewAction::Back,
        ReviewAction::Cancel,
    ]
    .into_iter()
    .enumerate()
    {
        state.step = SetupStep::Review;
        state.review_selection = index;
        state.finish = None;
        state.handle(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &settings,
            SetupMode::Init,
        );
        match action {
            ReviewAction::SaveAndExit => assert_eq!(Some(Finish::SaveAndExit), state.finish),
            ReviewAction::RepairCredential => unreachable!(),
            ReviewAction::Back => assert_eq!(SetupStep::Credential, state.step),
            ReviewAction::Cancel => assert_eq!(Some(Finish::Cancel), state.finish),
            ReviewAction::SaveAndContinue => unreachable!(),
        }
    }
}
