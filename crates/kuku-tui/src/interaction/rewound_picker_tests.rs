use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use kuku_client::{HistoryEntry, HistoryKind, HistoryPage, RewindGroup, RewindGroupPage};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::tests::{connected_app, press};
use crate::app::{Address, App, KeyboardFocus, Modal};
use crate::contextual::ContextControl;
use crate::jobs::Job;
use crate::menu::{ChoiceKind, PickerKind};
use crate::transcript::{MessageId, RewindGroupAction};

fn entry(id: u64, kind: HistoryKind, text: &str) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(1),
        kind,
        text: Some(text.into()),
        turn_summary: None,
        details: Default::default(),
    }
}

fn with_rewound_group() -> App {
    let mut app = connected_app();
    app.pane_mut().transcript.replace_recent(HistoryPage {
        entries: vec![entry(1, HistoryKind::User, "active message")],
        rewind_groups: vec![RewindGroup {
            rewind_event_id: 50,
            target_event_id: 0,
            agent: "main".into(),
            turn_number: Some(1),
        }],
        next_before: None,
    });
    app
}

pub(super) fn open_messages_with_f2(app: &mut App) {
    let controls = app.contextual_controls();
    let messages = controls
        .iter()
        .position(|control| matches!(control, ContextControl::Messages))
        .expect("Messages is an F2 contextual control");
    let current = app
        .pane()
        .contextual_focus
        .cursor
        .as_ref()
        .and_then(|cursor| controls.iter().position(|control| control == cursor));
    let count = match current {
        Some(current) => {
            let steps = (messages + controls.len() - current) % controls.len();
            if steps == 0 { controls.len() } else { steps }
        }
        None => messages + 1,
    };
    for _ in 0..count {
        press(app, KeyCode::F(2));
    }
    assert_eq!(
        Some(&ContextControl::Messages),
        app.pane().contextual_focus.cursor.as_ref()
    );
    press(app, KeyCode::Enter);
    assert!(
        matches!(&app.modal, Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Messages))
    );
}

pub(super) fn choose_group_action(app: &mut App, action: RewindGroupAction) {
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("Messages picker expected")
    };
    assert!(matches!(picker.kind, PickerKind::Messages));
    let target = picker
        .visible_choices()
        .iter()
        .position(|choice| {
            matches!(&choice.kind, ChoiceKind::RewindGroup { action: found, .. } if *found == action)
        })
        .expect("rewind action is available in Messages");
    loop {
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("Messages picker expected")
        };
        let selected = picker.selected;
        if selected == target {
            break;
        }
        press(
            app,
            if selected < target {
                KeyCode::Down
            } else {
                KeyCode::Up
            },
        );
    }
    press(app, KeyCode::Enter);
}

#[test]
fn f2_messages_separates_rewound_choices_and_preserves_restored_draft() {
    let mut app = connected_app();
    app.paste("first line\nsecond line");
    press(&mut app, KeyCode::Up);
    let original = app.active.clone();
    let draft = app.pane().draft.text().to_owned();
    let cursor = app.pane().draft.cursor();
    app.select(
        Address::new(original.session.clone(), "reviewer".into()),
        true,
        "reviewer".into(),
    );
    app.select(original, true, String::new());
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
    app.pane_mut().transcript.replace_recent(HistoryPage {
        entries: vec![entry(1, HistoryKind::User, "active message")],
        rewind_groups: vec![RewindGroup {
            rewind_event_id: 50,
            target_event_id: 0,
            agent: "main".into(),
            turn_number: Some(1),
        }],
        next_before: None,
    });

    open_messages_with_f2(&mut app);
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("Messages picker expected")
    };
    let choices = picker.visible_choices();
    let active_count = choices
        .iter()
        .filter(|choice| matches!(choice.kind, ChoiceKind::MessageEntry(_)))
        .count();
    let section = choices
        .iter()
        .position(|choice| matches!(choice.kind, ChoiceKind::RewindSection))
        .expect("read-only section is shown");
    assert_eq!(active_count, section);
    assert!(
        choices[section + 1..]
            .iter()
            .all(|choice| matches!(choice.kind, ChoiceKind::RewindGroup { .. }))
    );
    assert!(!choices.iter().any(|choice| matches!(
        &choice.kind,
        ChoiceKind::MessageEntry(message) if message.id == MessageId::Event(2)
    )));

    choose_group_action(&mut app, RewindGroupAction::Toggle);
    assert_eq!(KeyboardFocus::Transcript, app.pane().keyboard_focus);
    assert!(app.pane().transcript.is_reading());
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
    press(&mut app, KeyCode::Up);
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
    press(&mut app, KeyCode::Esc);
    assert!(app.composer_focused());
    assert!(!app.pane().contextual_focus.active);
    assert!(!app.rewind_armed);
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
}

#[test]
fn messages_with_only_rewound_groups_selects_the_toggle_not_the_section() {
    let mut app = connected_app();
    app.pane_mut().transcript.replace_recent(HistoryPage {
        entries: Vec::new(),
        rewind_groups: vec![RewindGroup {
            rewind_event_id: 50,
            target_event_id: 0,
            agent: "main".into(),
            turn_number: Some(1),
        }],
        next_before: None,
    });
    open_messages_with_f2(&mut app);
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("Messages picker expected")
    };
    assert!(matches!(
        picker.selected_choice(),
        Some(ChoiceKind::RewindGroup {
            action: RewindGroupAction::Toggle,
            ..
        })
    ));
    press(&mut app, KeyCode::Enter);
    assert!(
        app.pane()
            .transcript
            .rewind_group_choices()
            .iter()
            .any(|group| group.rewind_event_id == 50 && group.expanded)
    );
}

#[test]
fn messages_and_group_escape_do_not_arm_rewind_until_composer_escapes() {
    let mut app = with_rewound_group();
    app.pane_mut().exists = true;
    open_messages_with_f2(&mut app);
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert!(app.composer_focused());
    assert!(!app.rewind_armed);

    open_messages_with_f2(&mut app);
    choose_group_action(&mut app, RewindGroupAction::Toggle);
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert!(app.composer_focused());
    assert!(!app.rewind_armed);

    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert!(app.rewind_armed);
    press(&mut app, KeyCode::Esc);
    assert!(
        matches!(&app.modal, Some(Modal::Picker(picker)) if picker.kind == PickerKind::HistoryJump)
    );
}

#[test]
fn messages_revalidates_group_selection_and_routes_retry_and_more() {
    let mut app = with_rewound_group();
    open_messages_with_f2(&mut app);
    let Some(Modal::Picker(picker)) = app.modal.as_ref() else {
        panic!("Messages picker expected")
    };
    let ticket = picker.ticket.id;
    let Some(choice) = picker
        .visible_choices()
        .into_iter()
        .find(|choice| matches!(choice.kind, ChoiceKind::RewindGroup { .. }))
    else {
        panic!("rewind choice")
    };
    let ChoiceKind::RewindGroup {
        mut selection,
        rewind_event_id,
        action,
    } = choice.kind.clone()
    else {
        panic!("rewind choice")
    };
    selection.generation = selection.generation.wrapping_add(1);
    let stale = ChoiceKind::RewindGroup {
        selection,
        rewind_event_id,
        action,
    };
    app.choose(PickerKind::Messages, stale, ticket);
    assert!(
        !app.effects
            .iter()
            .any(|job| matches!(job, Job::RewindGroupHistory(_, _)))
    );
    assert!(
        app.pane()
            .transcript
            .rewind_group_choices()
            .iter()
            .all(|group| !group.expanded)
    );

    open_messages_with_f2(&mut app);
    choose_group_action(&mut app, RewindGroupAction::Toggle);
    let request = app
        .effects
        .iter()
        .find_map(|job| {
            if let Job::RewindGroupHistory(_, request) = job {
                Some(request.clone())
            } else {
                None
            }
        })
        .expect("first page request");
    app.effects.clear();
    assert!(
        app.pane_mut()
            .transcript
            .apply_rewind_group_page(&request, Err("offline".into()))
    );

    open_messages_with_f2(&mut app);
    choose_group_action(&mut app, RewindGroupAction::Retry);
    let retry = app
        .effects
        .iter()
        .find_map(|job| {
            if let Job::RewindGroupHistory(_, request) = job {
                Some(request.clone())
            } else {
                None
            }
        })
        .expect("retry request from Messages");
    assert_eq!(None, retry.before);
    app.effects.clear();
    assert!(app.pane_mut().transcript.apply_rewind_group_page(
        &retry,
        Ok(RewindGroupPage {
            entries: vec![entry(2, HistoryKind::User, "hidden earlier entry")],
            next_before: Some(2),
        }),
    ));

    open_messages_with_f2(&mut app);
    choose_group_action(&mut app, RewindGroupAction::More);
    let more = app
        .effects
        .iter()
        .find_map(|job| {
            if let Job::RewindGroupHistory(_, request) = job {
                Some(request.clone())
            } else {
                None
            }
        })
        .expect("load-earlier request from Messages");
    assert_eq!(Some(2), more.before);
}

#[test]
fn messages_rewind_group_choice_activates_on_a_mouse_click() {
    let mut app = with_rewound_group();
    open_messages_with_f2(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let buffer = terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap()
        .buffer;
    let width = usize::from(buffer.area.width);
    let (row, column) = buffer
        .content
        .chunks(width)
        .enumerate()
        .find_map(|(row, cells)| {
            let line = cells
                .iter()
                .map(|cell| cell.symbol())
                .collect::<Vec<_>>()
                .join("");
            line.find("Expand group")
                .map(|byte| (row as u16, line[..byte].chars().count() as u16))
        })
        .expect("rendered read-only group choice");
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.event(
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }),
            buffer,
        );
    }
    assert!(
        app.pane()
            .transcript
            .rewind_group_choices()
            .iter()
            .any(|group| group.rewind_event_id == 50 && group.expanded)
    );
}
