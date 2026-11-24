use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{app, key, mouse, render, screen, text_position};
use crate::app::{App, Modal};
use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::model_default::SAVE_SCOPE;

fn model_default_confirmation(header: String) -> App {
    let mut app = app();
    let picker = app.picker(
        PickerKind::ModelDefault,
        vec![
            Choice::new(
                "Save as default",
                "Write the confirmed config change",
                ChoiceKind::ConfirmModelDefault,
            ),
            Choice::new(
                "Cancel",
                "No files are changed",
                ChoiceKind::CancelModelDefault,
            ),
        ],
    );
    picker.header = header;
    picker.selected = 1;
    app
}

fn normalized_screen(screen: &str) -> String {
    screen.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_screen(screen: &str) -> String {
    screen
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

#[test]
fn model_default_confirmation_shows_scope_and_actions_at_60x20_and_normal_size() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    let header = format!(
        "Connection/model: openai/gpt-4\nConfig: {}\nDefault tier: balanced",
        config_path.display()
    );
    let mut app = model_default_confirmation(header);

    let minimum = normalized_screen(&screen(&mut app, 60, 20));
    assert!(minimum.contains(SAVE_SCOPE));
    assert!(minimum.contains("Connection/model: openai/gpt-4"));
    assert!(compact_screen(&minimum).contains(&compact_screen(&config_path.display().to_string())));
    assert!(minimum.contains("Default tier: balanced"));
    assert!(minimum.contains("Save as default"));
    assert!(minimum.contains("Cancel"));

    let normal = normalized_screen(&screen(&mut app, 100, 30));
    assert!(normal.contains(SAVE_SCOPE));
    assert!(normal.contains("Save as default"));
    assert!(normal.contains("Cancel"));
}

#[test]
fn model_default_confirmation_scrolls_long_paths_without_hiding_scope_or_actions() {
    let mut saw_wrapped_filename = false;
    let mut saw_unwrapped_filename = false;
    for temp_component_length in 0..=60 {
        let mut config_path = format!(
            "/var/folders/zz/zyxvpxvq6csfxvn_n00000000000000/T/{}",
            "x".repeat(temp_component_length)
        );
        for index in 0..24 {
            config_path.push_str(&format!("/config-segment-{index:02}"));
        }
        config_path.push_str("/final-config.toml");
        let header = format!(
            "Connection/model: openai/gpt-4\nConfig: {config_path}\nDefault tier: balanced"
        );
        let mut app = model_default_confirmation(header);
        let first_page = normalized_screen(&screen(&mut app, 60, 20));
        assert!(compact_screen(&first_page).contains(&compact_screen(SAVE_SCOPE)));
        assert!(first_page.contains("Save as default"));
        assert!(first_page.contains("Cancel"));

        app.event(
            Event::Key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
            &Buffer::empty(Rect::new(0, 0, 60, 20)),
        );
        let last_page = normalized_screen(&screen(&mut app, 60, 20));
        assert!(
            compact_screen(&last_page).contains("final-config.toml"),
            "prefix component length {temp_component_length}:\n{last_page}"
        );
        if last_page.contains("final-config.toml") {
            saw_unwrapped_filename = true;
        } else {
            saw_wrapped_filename = true;
        }
        assert!(compact_screen(&last_page).contains(&compact_screen(SAVE_SCOPE)));
        assert!(last_page.contains("Save as default"));
        assert!(last_page.contains("Cancel"));
    }
    assert!(
        saw_wrapped_filename,
        "prefix sweep should wrap within the filename"
    );
    assert!(
        saw_unwrapped_filename,
        "prefix sweep should include an unwrapped filename"
    );
}

#[test]
fn model_default_confirmation_requires_full_minimum_frame_and_escape_is_safe() {
    let directory = tempfile::tempdir().unwrap();
    let header = format!(
        "Connection/model: openai/gpt-4\nConfig: {}\nDefault tier: balanced",
        directory.path().join("config.toml").display()
    );
    for (width, height) in [(59, 20), (60, 19), (30, 10)] {
        let mut app = model_default_confirmation(header.clone());
        let rendered = normalized_screen(&screen(&mut app, width, height));
        assert!(!rendered.contains("Save as default"), "{width}x{height}");
        assert!(rendered.contains("Cancel"), "{width}x{height}");
        assert!(
            compact_screen(&rendered).contains("resizetoenableSave"),
            "{width}x{height}"
        );
        let Some(crate::app::Modal::Picker(picker)) = &app.modal else {
            panic!("confirmation should remain open at {width}x{height}");
        };
        assert_eq!(1, picker.choice_count(), "{width}x{height}");

        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(
            app.modal.is_none(),
            "Escape should cancel at {width}x{height}"
        );
        assert!(
            !app.effects
                .iter()
                .any(|job| matches!(job, Job::SaveDefault { .. }))
        );
    }
}

#[test]
fn model_default_loading_and_error_states_do_not_render_confirmation_actions() {
    let mut loading = app();
    let picker = loading.picker(PickerKind::ModelDefault, Vec::new());
    picker.loading = true;
    picker.header = SAVE_SCOPE.into();
    let loading_screen = normalized_screen(&screen(&mut loading, 60, 20));
    assert!(loading_screen.contains("Loading"));
    assert!(!loading_screen.contains("Save as default"));

    let mut failed = app();
    let picker = failed.picker(PickerKind::ModelDefault, Vec::new());
    picker.problem = Some("preparation failed".into());
    picker.header = SAVE_SCOPE.into();
    let error_screen = normalized_screen(&screen(&mut failed, 60, 20));
    assert!(error_screen.contains("preparation failed"));
    assert!(!error_screen.contains("Save as default"));
}

#[test]
fn model_default_pointer_actions_survive_redraw_between_press_and_release() {
    for label in ["Save as default", "Cancel"] {
        let mut app = model_default_confirmation("Connection/model: openai/gpt-4".into());
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        let position = text_position(buffer, label);
        let left = MouseButton::Left;
        let modifiers = KeyModifiers::NONE;
        app.event(
            mouse(MouseEventKind::Down(left), position, modifiers),
            buffer,
        );

        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        app.event(mouse(MouseEventKind::Up(left), position, modifiers), buffer);

        assert!(app.modal.is_none(), "{label} click should be applied");
    }
}

#[test]
fn model_default_resize_below_minimum_invalidates_save_hits_without_queuing_save() {
    for (width, height) in [(59, 20), (60, 19), (30, 10)] {
        let mut app = model_default_confirmation("Connection/model: openai/gpt-4".into());
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        let save_position = text_position(buffer, "Save as default");
        app.event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                save_position,
                KeyModifiers::NONE,
            ),
            buffer,
        );

        let mut small_terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let small_buffer = small_terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        assert!(matches!(
            &app.modal,
            Some(Modal::Picker(picker)) if picker.choice_count() == 1
        ));
        app.event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                save_position,
                KeyModifiers::NONE,
            ),
            small_buffer,
        );
        assert!(matches!(
            &app.modal,
            Some(Modal::Picker(picker)) if picker.choice_count() == 1
        ));
        assert!(
            !app.effects
                .iter()
                .any(|job| matches!(job, Job::SaveDefault { .. }))
        );

        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        assert!(matches!(
            &app.modal,
            Some(Modal::Picker(picker))
                if picker.choice_count() == 2
                    && matches!(picker.selected_choice(), Some(ChoiceKind::CancelModelDefault))
        ));
        assert_eq!(60, buffer.area.width);
        assert_eq!(20, buffer.area.height);
    }
}
