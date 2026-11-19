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
            ReviewAction::SaveAndContinue => assert_eq!(Some(Finish::SaveAndContinue), state.finish),
            ReviewAction::SaveAndExit => assert_eq!(Some(Finish::SaveAndExit), state.finish),
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
            ReviewAction::Back => assert_eq!(SetupStep::Credential, state.step),
            ReviewAction::Cancel => assert_eq!(Some(Finish::Cancel), state.finish),
            ReviewAction::SaveAndContinue => unreachable!(),
        }
    }
}
