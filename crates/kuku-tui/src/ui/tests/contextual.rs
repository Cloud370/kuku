use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::{Action, HistoryPage, Input, QueuedInput, Receipt, Timing};
use serde_json::json;

use super::{app, click, key, render, screen};
use crate::app::Modal;
use crate::contextual::{ContextControl, PendingCommandIdentity};
use crate::jobs::{Job, Update};
use crate::menu::PickerKind;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn load_snapshot(app: &mut crate::app::App, status: &str, with_input: bool) {
    let mut snapshot = super::snapshot(app, status, json!(null), json!(null));
    if with_input {
        snapshot.agent.pending_inputs = 1;
        snapshot.pending.push(QueuedInput {
            input: Input::new("queued-command", app.active.agent.clone(), "waiting"),
            receipt: Receipt {
                command_id: "queued-command".into(),
                event_id: 53,
                agent: app.active.agent.clone(),
            },
        });
    }
    app.update(Update::Loaded(
        app.selection(),
        snapshot,
        HistoryPage {
            entries: Vec::new(),
            next_before: None,
        },
    ));
    app.effects.clear();
}

fn pending_send(app: &mut crate::app::App) -> kuku_client::Command {
    app.pane_mut().exists = true;
    app.pane_mut().draft.set_text("pending send".into());
    app.send(Timing::Steer);
    let Some(Job::Save(command)) = app.effects.pop() else {
        panic!("send command should be saved")
    };
    app.update(Update::Pending(Ok(vec![command.clone()])));
    command
}

#[test]
fn clicking_restart_only_focuses_it_and_enter_preserves_the_draft() {
    let mut app = app();
    app.pane_mut().exists = true;
    app.pane_mut().draft.set_text("unsent draft".into());
    let mut terminal = Terminal::new(TestBackend::new(82, 20)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let hit = app
        .contextual_hits
        .iter()
        .find(|hit| matches!(hit.control, ContextControl::Restart { .. }))
        .unwrap()
        .area;

    click(&mut app, &buffer, (hit.x, hit.y));
    assert!(app.pane().contextual_focus.active);
    assert!(!app.restarting);
    assert_eq!("unsent draft", app.pane().draft.text());

    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.restarting);
    assert_eq!("unsent draft", app.pane().draft.text());
}

#[test]
fn narrow_layout_keeps_contextual_service_action_keyboard_reachable() {
    let mut app = app();
    app.pane_mut().exists = true;
    let view = screen(&mut app, 24, 12);
    assert!(!view.is_empty());
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.pane().contextual_focus.active);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.restarting);
}

#[test]
fn continue_requires_stopped_work_with_pending_inputs() {
    let mut stopped = app();
    load_snapshot(&mut stopped, "stopped", true);
    assert!(stopped
        .contextual_controls()
        .iter()
        .any(|control| matches!(control, ContextControl::Continue { .. })));

    let mut no_input = app();
    load_snapshot(&mut no_input, "stopped", false);
    assert!(!no_input
        .contextual_controls()
        .iter()
        .any(|control| matches!(control, ContextControl::Continue { .. })));

    let mut running = app();
    load_snapshot(&mut running, "running", true);
    assert!(!running
        .contextual_controls()
        .iter()
        .any(|control| matches!(control, ContextControl::Continue { .. })));
    assert!(running
        .contextual_controls()
        .iter()
        .any(|control| matches!(control, ContextControl::Interrupt { .. })));

    let mut cleaning = app();
    load_snapshot(&mut cleaning, "interrupted", true);
    cleaning.pane_mut().snapshot.as_mut().unwrap().cleaning_up = true;
    assert!(!cleaning
        .contextual_controls()
        .iter()
        .any(|control| matches!(control, ContextControl::Continue { .. })));
}

#[test]
fn continue_enter_preserves_composer_draft_and_revalidates_input_identity() {
    let mut current = app();
    load_snapshot(&mut current, "stopped", true);
    current
        .pane_mut()
        .draft
        .set_text("unsent composer draft".into());
    key(&mut current, KeyCode::F(2), KeyModifiers::NONE);
    assert!(current.pane().contextual_focus.active);
    key(&mut current, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!("unsent composer draft", current.pane().draft.text());
    assert!(current.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Advance)
    )));

    let mut stale = app();
    load_snapshot(&mut stale, "stopped", true);
    key(&mut stale, KeyCode::F(2), KeyModifiers::NONE);
    stale.pane_mut().snapshot.as_mut().unwrap().pending[0]
        .receipt
        .command_id = "replacement-command".into();
    key(&mut stale, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!stale.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Advance)
    )));
}

#[test]
fn focused_escape_dismisses_without_interrupting_and_ordinary_escape_still_interrupts() {
    let mut app = app();
    load_snapshot(&mut app, "running", false);
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.pane().contextual_focus.active);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.pane().contextual_focus.active);
    assert!(!app.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Cancel { .. })
    )));

    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Cancel { execution_id: 49 })
    )));
}

#[test]
fn pending_recovery_requires_the_same_saved_command_identity() {
    let mut current = app();
    let command = pending_send(&mut current);
    let control = current
        .contextual_controls()
        .into_iter()
        .find(|control| matches!(control, ContextControl::PendingRecovery { .. }))
        .unwrap();
    assert!(matches!(&control,
        ContextControl::PendingRecovery { commands }
            if commands == &[PendingCommandIdentity {
                target: command.target.clone(),
                command_id: command.command_id.clone(),
            }]
    ));
    assert!(current.focus_context_control(control));
    key(&mut current, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&current.modal,
        Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Pending)
    ));

    let mut stale = app();
    let command = pending_send(&mut stale);
    let control = stale
        .contextual_controls()
        .into_iter()
        .find(|control| matches!(control, ContextControl::PendingRecovery { .. }))
        .unwrap();
    assert!(stale.focus_context_control(control));
    stale.pending.remove(&command.command_id);
    key(&mut stale, KeyCode::Enter, KeyModifiers::NONE);
    assert!(stale.modal.is_none());
}
