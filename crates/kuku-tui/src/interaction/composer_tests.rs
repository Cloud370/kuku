use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use kuku_client::{Action, HistoryKind, HistoryPage, OutputReference, Timing};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use ratatui::Terminal;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, KeyboardFocus, Modal};
use crate::jobs::{Job, Update};
use crate::menu::PickerKind;
use crate::setup::ThinkingDefaults;

use super::tests::{connected_app, insert_message, press};

fn load_model(app: &mut App, applied: Option<&str>, next: Option<&str>) {
    let selection = |effort| {
        json!({
            "connection": "fixture",
            "protocol": "openai-responses",
            "model": "gemini-4-neon",
            "context_window_tokens": 100_000,
            "max_output_tokens": 4_000,
            "settings": {"reasoning_effort": effort},
            "capabilities": {
                "reasoning_efforts": ["low", "medium", "high", "high", "max"],
                "reasoning_default": {"value": "max", "source": "model_list"}
            },
            "tools": [],
        })
    };
    let config = |effort| json!({"tier":"fixture-model", "reasoning_effort":effort});
    let snapshot = serde_json::from_value(json!({
        "session_id": app.active.session,
        "workspace": app.workspace,
        "cursor": 52,
        "pending": [],
        "agents": [],
        "selection": selection(applied.filter(|effort| *effort != "provider-default")),
        "pending_selection": next.map(|effort| selection(Some(effort))),
        "agent": {
            "address": app.active.agent,
            "config": config(next.or(applied)),
            "active_config": next.map(|_| config(applied)),
            "status": "running",
            "turn": 10,
            "pending_inputs": 0,
            "execution_id": 52,
            "usage": {"model_requests":0,"unreported_requests":0},
        }
    }))
    .unwrap();
    app.update(Update::Loaded(
        app.selection(),
        Box::new(snapshot),
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(app.selection()));
    app.effects.clear();
}

fn action_picker(app: &mut App, query: &str) {
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    press(app, KeyCode::Char('/'));
    app.paste(query);
}

fn frame(app: &mut App, width: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, 28)).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, app))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buffer: &Buffer) -> Vec<String> {
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

fn position(buffer: &Buffer, text: &str) -> (u16, u16) {
    lines(buffer)
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(text)
                .map(|index| (line[..index].width() as u16, row as u16))
        })
        .unwrap_or_else(|| panic!("Missing {text} in\n{}", lines(buffer).join("\n")))
}

#[test]
fn jobs_alias_preserves_arguments_through_completion_paste_and_execution() {
    let mut app = connected_app();
    app.pane_mut().exists = true;
    press(&mut app, KeyCode::Char('/'));
    for character in "job  cancel JOB-8".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    press(&mut app, KeyCode::Tab);
    assert_eq!("/jobs  cancel JOB-8", app.pane().draft.text());
    assert!(app.modal.is_none());
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.effects.last(), Some(Job::CancelBackground(selection, id))
        if *selection == app.selection() && id == "JOB-8")
    );
    assert!(app.alias_hint.is_none());

    app.paste("/job cancel JOB-9");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.effects.last(), Some(Job::CancelBackground(_, id)) if id == "JOB-9"));
    assert_eq!("/job → /jobs", app.alias_hint.take().unwrap().text);
    app.paste("/job cancel JOB-10");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.effects.last(), Some(Job::CancelBackground(_, id)) if id == "JOB-10"));
    assert!(app.alias_hint.is_none());

    app.effects.clear();
    app.paste("/job cancel");
    press(&mut app, KeyCode::Enter);
    assert_eq!("/job cancel", app.pane().draft.text());
    assert_eq!("Use /jobs or /jobs cancel <id>", app.notice.unwrap().text);
    assert!(app.effects.is_empty());
}

#[test]
fn bare_exit_words_only_open_local_confirmation_on_both_submit_keys() {
    for (text, modifiers) in [
        ("  exit  ", KeyModifiers::NONE),
        ("  quit ", KeyModifiers::ALT),
    ] {
        let mut app = connected_app();
        load_model(&mut app, Some("low"), None);
        app.paste(text);
        press(&mut app, KeyCode::Left);
        let cursor = app.pane().draft.cursor();
        app.key(KeyEvent::new(KeyCode::Enter, modifiers));
        assert!(matches!(app.modal, Some(Modal::ExitConfirmation)));
        app.key(KeyEvent {
            kind: KeyEventKind::Repeat,
            ..KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        });
        assert!(!app.quit);
        for width in [60, 120] {
            let buffer = frame(&mut app, width);
            let (x, y) = position(&buffer, "Exit kuku? Y exit · Esc edit");
            assert_eq!(app.composer_area.x, x);
            assert!(y < app.composer_area.y && app.composer_area.y - y <= 4);
        }
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        assert!(!app.quit);
        assert_eq!(text, app.pane().draft.text());
        assert_eq!(cursor, app.pane().draft.cursor());
        assert_eq!(Some(52), app.pane().execution());
        assert!(app.pending.is_empty() && app.effects.is_empty());

        app.key(KeyEvent::new(KeyCode::Enter, modifiers));
        press(&mut app, KeyCode::Char('Y'));
        assert!(app.quit);
        assert!(app.pending.is_empty() && app.effects.is_empty());
    }
}

#[test]
fn exit_phrases_multiline_and_attached_input_still_submit() {
    for (text, attached) in [
        ("explain exit", false),
        ("quit\n# code", false),
        ("EXIT", false),
        ("/unrecognized", false),
        ("/quit with arguments", false),
        ("quit", true),
    ] {
        let mut app = connected_app();
        app.paste(text);
        if attached {
            app.pane_mut().value_attachments.push((
                "notes".into(),
                OutputReference {
                    id: "saved-notes".into(),
                    offset: 0,
                    bytes: 12,
                },
            ));
        }
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert!(
            matches!(app.effects.as_slice(), [Job::Save(command)]
            if matches!(&command.action, Action::Send { text: sent, timing: Timing::NextTurn, values, .. }
                if sent == text && values.len() == usize::from(attached))),
            "{text}"
        );
    }
    for alias in ["/quit", "/exit", "/q"] {
        let mut app = connected_app();
        app.paste(alias);
        press(&mut app, KeyCode::Enter);
        assert!(app.quit && app.modal.is_none());
        assert!(app.alias_hint.is_none());
        assert!(app.effects.is_empty());
    }
}

#[test]
fn composer_lists_keep_alignment_draft_selection_and_reading_position() {
    for width in [60, 120] {
        let mut app = connected_app();
        load_model(&mut app, Some("low"), Some("medium"));
        for id in 1..30 {
            insert_message(
                &mut app,
                id,
                HistoryKind::Assistant,
                &format!("Reading line {id}"),
                Default::default(),
            );
        }
        app.paste("draft stays in place");
        press(&mut app, KeyCode::Left);
        let cursor = app.pane().draft.cursor();
        let baseline = frame(&mut app, width);
        assert!(lines(&baseline)
            .iter()
            .any(|line| line.contains("Next model request") && line.contains("· Medium")));
        app.scroll(-6);
        assert_eq!(KeyboardFocus::Transcript, app.pane().keyboard_focus);
        let anchor = app.pane().transcript.reading_anchor().cloned();
        assert!(anchor.is_some());
        let (column, row) = position(&baseline, "draft stays in place");
        let button = MouseButton::Left;
        for (kind, col) in [
            (MouseEventKind::Down(button), column),
            (MouseEventKind::Drag(button), column + 4),
        ] {
            app.event(
                Event::Mouse(MouseEvent {
                    kind,
                    column: col,
                    row,
                    modifiers: KeyModifiers::NONE,
                }),
                &baseline,
            );
        }
        action_picker(&mut app, "models");
        let suggestions = frame(&mut app, width);
        let label = lines(&suggestions)
            .into_iter()
            .find(|line| line.contains("/model ·"))
            .unwrap();
        assert_eq!(
            (column, row),
            position(&suggestions, "draft stays in place")
        );
        assert_eq!(column, position(&suggestions, "/model ·").0);
        assert!(label.trim_end().width() <= 64 + usize::from(column));
        assert!(suggestions[(column, row)]
            .modifier
            .contains(Modifier::REVERSED));
        press(&mut app, KeyCode::Esc);

        action_picker(&mut app, "thinking");
        press(&mut app, KeyCode::Enter);
        let Some(Job::ThinkingDefaults(ticket, ..)) = app.effects.last() else {
            panic!("The inherited value must resolve without the Agent override");
        };
        app.update(Update::ThinkingDefaults(
            *ticket,
            ThinkingDefaults {
                inherited: Some(
                    serde_json::from_value::<kuku::ProviderSettings>(
                        json!({"reasoning_effort":"low"}),
                    )
                    .unwrap(),
                ),
                save: None,
            },
        ));
        press(&mut app, KeyCode::Down);
        let thinking = frame(&mut app, width);
        let text = lines(&thinking).join("\n");
        assert_eq!(
            column,
            position(&thinking, "Use default · Model default · Low").0
        );
        assert!(text.contains("Medium ✓ · Agent override"), "{text}");
        assert!(text.contains("Provider declares · Max"), "{text}");
        assert!(text.contains("gemini-4-neon · Low"), "{text}");
        assert!(
            text.contains("Thinking · gemini-4-neon · Next request"),
            "{text}"
        );
        assert_eq!(1, text.matches("High").count());
        press(&mut app, KeyCode::Char('d'));
        assert!(
            matches!(&app.modal, Some(Modal::Picker(picker)) if picker.kind == PickerKind::Thinking
            && picker.thinking.as_ref().is_some_and(|thinking| thinking.confirm.is_none()))
        );
        press(&mut app, KeyCode::Esc);

        assert_eq!(KeyboardFocus::Transcript, app.pane().keyboard_focus);
        assert_eq!(anchor.as_ref(), app.pane().transcript.reading_anchor());
        assert_eq!(cursor, app.pane().draft.cursor());
        assert_eq!("draft stays in place", app.pane().draft.text());
        assert!(app.pending.is_empty());
    }
}

#[test]
fn cycling_respects_pending_model_settings_and_changes_only_the_active_agent() {
    let mut app = connected_app();
    load_model(&mut app, Some("low"), Some("high"));
    app.paste("draft to keep");
    press(&mut app, KeyCode::Left);
    app.focus_history();
    let cursor = app.pane().draft.cursor();
    let cycle = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
    app.key(cycle);
    assert!(matches!(app.effects.as_slice(), [Job::Save(command)]
        if command.target == app.selection().target &&
        matches!(&command.action, Action::Configure { config }
            if config.reasoning_effort.as_deref() == Some("max") && config.tier.as_deref() == Some("fixture-model"))));
    app.key(cycle);
    assert_eq!(1, app.effects.len());
    assert_eq!(KeyboardFocus::Transcript, app.pane().keyboard_focus);
    assert_eq!(cursor, app.pane().draft.cursor());
    assert_eq!("draft to keep", app.pane().draft.text());
    assert_eq!(
        Some("low"),
        app.pane().effective_model_identity().unwrap().2
    );
}

#[test]
fn use_default_clears_a_legacy_override_without_reconfiguring_the_route() {
    let mut app = connected_app();
    load_model(&mut app, Some("provider-default"), None);
    app.open_thinking();
    let screen = frame(&mut app, 60);
    let text = lines(&screen).join("\n");
    assert!(text.contains("gemini-4-neon · Default"), "{text}");
    assert!(text.contains("Current · Default"), "{text}");
    assert!(!text.contains("Provider default"), "{text}");
    app.effects.clear();
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.effects.as_slice(), [Job::Save(command), Job::ClosePicker]
        if matches!(&command.action, Action::Configure { config }
            if config.reasoning_effort.is_none() && config.tier.as_deref() == Some("fixture-model")))
    );
}
