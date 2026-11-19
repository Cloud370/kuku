use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::Error;
use kuku_client::{Action, HistoryPage, Input, QueuedInput, Receipt, Timing};
use serde_json::json;

use super::{app, click, key, render, screen};
use crate::app::{KeyboardFocus, Modal};
use crate::config::StatusLineSettings;
use crate::contextual::{ContextControl, PendingCommandIdentity, PendingCommandKind};
use crate::jobs::{Job, Update};
use crate::menu::PickerKind;
use crate::statusline::StatusLine;
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
            rewind_groups: Vec::new(),
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

    click(&mut app, buffer, (hit.x, hit.y));
    assert!(app.pane().contextual_focus.active);
    assert!(!app.restarting);
    assert_eq!("unsent draft", app.pane().draft.text());

    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.restarting);
    assert_eq!("unsent draft", app.pane().draft.text());
}

#[test]
fn clicking_stopped_continue_focuses_it_until_enter_advances() {
    let mut app = app();
    load_snapshot(&mut app, "stopped", true);
    let mut terminal = Terminal::new(TestBackend::new(82, 20)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let hit = app
        .contextual_hits
        .iter()
        .find(|hit| matches!(hit.control, ContextControl::Continue { .. }))
        .unwrap()
        .area;

    click(&mut app, buffer, (hit.x, hit.y));
    assert!(matches!(
        app.pane().contextual_focus.cursor,
        Some(ContextControl::Continue { .. })
    ));
    assert!(app.pane().contextual_focus.active);
    assert!(!app.effects.iter().any(|effect| matches!(effect,
        Job::Save(command) if matches!(command.action, Action::Advance)
    )));

    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.effects.iter().any(|effect| matches!(effect,
        Job::Save(command) if matches!(command.action, Action::Advance)
    )));
}

#[test]
fn clicking_running_stop_focuses_it_until_enter_interrupts() {
    let mut app = app();
    load_snapshot(&mut app, "running", false);
    let mut terminal = Terminal::new(TestBackend::new(82, 20)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let hit = app
        .contextual_hits
        .iter()
        .find(|hit| matches!(hit.control, ContextControl::Interrupt { .. }))
        .unwrap()
        .area;

    click(&mut app, buffer, (hit.x, hit.y));
    assert!(matches!(
        app.pane().contextual_focus.cursor,
        Some(ContextControl::Interrupt {
            execution_id: 49,
            ..
        })
    ));
    assert!(app.pane().contextual_focus.active);
    assert!(!app.effects.iter().any(|effect| matches!(effect,
        Job::Save(command) if matches!(command.action, Action::Cancel { .. })
    )));

    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.effects.iter().any(|effect| matches!(effect,
        Job::Save(command) if matches!(command.action, Action::Cancel { execution_id: 49 })
    )));
}

#[test]
fn clicking_previous_agent_header_focuses_the_registered_hit_area() {
    let mut app = app();
    load_snapshot(&mut app, "idle", false);
    let previous = app.active.clone();
    let next = crate::app::Address::new(previous.session.clone(), "writer".into());
    app.select(next, true, "Writer".into());
    load_snapshot(&mut app, "idle", false);

    let mut terminal = Terminal::new(TestBackend::new(82, 20)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let hit = app
        .contextual_hits
        .iter()
        .find(|hit| matches!(hit.control, ContextControl::PreviousAgent { .. }))
        .unwrap()
        .area;

    click(&mut app, buffer, (hit.x, hit.y));
    assert!(matches!(&app.pane().contextual_focus.cursor,
        Some(ContextControl::PreviousAgent { target }) if target == &previous));
    assert!(app.pane().contextual_focus.active);
    assert_eq!("writer", app.active.agent);

    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(previous, app.active);
}

#[test]
fn contextual_cycle_cursor_is_retained_per_agent_across_navigation() {
    let mut app = app();
    load_snapshot(&mut app, "idle", false);
    let main = app.active.clone();
    let writer = crate::app::Address::new(main.session.clone(), "writer".into());
    app.select(writer.clone(), true, "Writer".into());
    load_snapshot(&mut app, "idle", false);

    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(matches!(&app.pane().contextual_focus.cursor,
        Some(ContextControl::PreviousAgent { target }) if target == &main));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(main, app.active);

    app.select(writer, true, "Writer".into());
    assert!(!app.pane().contextual_focus.active);
    assert!(matches!(&app.pane().contextual_focus.cursor,
        Some(ContextControl::PreviousAgent { target }) if target == &main));
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(matches!(
        app.pane().contextual_focus.cursor,
        Some(ContextControl::Restart { .. })
    ));
}

#[test]
fn narrow_layout_keeps_contextual_service_action_keyboard_reachable() {
    let mut app = app();
    app.pane_mut().exists = true;
    let view = screen(&mut app, 24, 12);
    assert!(!view.is_empty());
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.pane().contextual_focus.active);
    let focused = screen(&mut app, 24, 12);
    assert!(focused.contains("Restart"));
    assert!(focused.contains("Enter act"));
    assert!(focused.contains("Esc back"));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.restarting);
}

#[test]
fn focused_control_hints_render_with_default_and_custom_statuslines() {
    let mut ordinary = app();
    ordinary.pane_mut().exists = true;
    let ordinary_view = screen(&mut ordinary, 82, 20);
    assert!(ordinary_view.contains("F2 Restart · Enter"));

    let mut desktop = app();
    desktop.pane_mut().exists = true;
    key(&mut desktop, KeyCode::F(2), KeyModifiers::NONE);
    let default_view = screen(&mut desktop, 82, 20);
    assert!(default_view.contains("Restart"));
    assert!(default_view.contains("Enter act"));
    assert!(default_view.contains("Esc back"));

    let mut running = app();
    load_snapshot(&mut running, "running", false);
    key(&mut running, KeyCode::F(2), KeyModifiers::NONE);
    let running_view = screen(&mut running, 82, 20);
    assert!(running_view.contains("Esc stop"));
    assert!(!running_view.contains("Esc back"));

    let mut custom = app();
    custom.pane_mut().exists = true;
    custom.statusline_settings = StatusLineSettings::in_memory(StatusLine::parse("input").unwrap());
    key(&mut custom, KeyCode::F(2), KeyModifiers::NONE);
    let custom_view = screen(&mut custom, 82, 20);
    assert!(custom_view.contains("Restart"));
    assert!(custom_view.contains("Enter act"));
    assert!(custom_view.contains("Esc back"));
}

#[test]
fn continue_requires_pending_inputs_for_stopped_but_not_interrupted_work() {
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

    let mut interrupted = app();
    load_snapshot(&mut interrupted, "interrupted", false);
    interrupted
        .pane_mut()
        .snapshot
        .as_mut()
        .unwrap()
        .agent
        .last_error = Some("provider returned HTTP 400".into());
    assert!(interrupted
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
    cleaning
        .pane_mut()
        .snapshot
        .as_mut()
        .unwrap()
        .agent
        .last_error = Some("provider returned HTTP 400".into());
    cleaning.pane_mut().snapshot.as_mut().unwrap().cleaning_up = true;
    assert!(!cleaning
        .contextual_controls()
        .iter()
        .any(|control| matches!(control, ContextControl::Continue { .. })));
}

#[test]
fn interrupted_continue_with_empty_queue_advances() {
    let mut interrupted = app();
    load_snapshot(&mut interrupted, "interrupted", false);
    interrupted
        .pane_mut()
        .snapshot
        .as_mut()
        .unwrap()
        .agent
        .last_error = Some("provider returned HTTP 400".into());

    key(&mut interrupted, KeyCode::F(2), KeyModifiers::NONE);
    assert!(matches!(
        interrupted.pane().contextual_focus.cursor.as_ref(),
        Some(ContextControl::Continue { inputs, .. }) if inputs.is_empty()
    ));
    key(&mut interrupted, KeyCode::Enter, KeyModifiers::NONE);
    assert!(interrupted.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Advance)
    )));
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
fn focused_escape_interrupts_a_running_task_and_returns_to_composer() {
    let mut app = app();
    load_snapshot(&mut app, "running", false);
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.pane().contextual_focus.active);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.pane().contextual_focus.active);
    assert!(app.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Cancel { execution_id: 49 })
    )));
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
}

#[test]
fn idle_focused_escape_returns_to_composer_without_side_effects() {
    let mut app = app();
    load_snapshot(&mut app, "idle", false);
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.pane().contextual_focus.active);
    app.effects.clear();
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.pane().contextual_focus.active);
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    assert!(app.effects.is_empty());
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
                kind: PendingCommandKind::Send,
            }]
    ));
    assert!(current.focus_context_control(control));
    let focused = screen(&mut current, 82, 20);
    assert!(focused.contains("Unconfirmed send"));
    assert!(focused.contains("Enter act"));
    assert!(focused.contains("Esc back"));
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

#[test]
fn failed_configure_and_other_saved_action_open_recovery_without_a_send() {
    let mut app = app();
    for (id, action) in [
        (
            "configure-command",
            Action::Configure {
                config: Box::default(),
            },
        ),
        (
            "create-agent-command",
            Action::CreateAgent {
                config: Box::default(),
            },
        ),
    ] {
        app.issue(app.active.clone(), action);
        let Some(Job::Save(mut command)) = app.effects.pop() else {
            panic!("saved action should be queued")
        };
        command.command_id = id.into();
        app.update(Update::Saved(command.clone(), Ok(())));
        app.update(Update::Sent(
            command.command_id.clone(),
            Err(Error::Service {
                status: 503,
                code: "provider_error".into(),
                error_message: "provider failed before confirming the action".into(),
            }),
        ));
        app.effects.clear();
    }
    let control = app
        .contextual_controls()
        .into_iter()
        .find(|control| matches!(control, ContextControl::PendingRecovery { .. }))
        .unwrap();
    assert!(matches!(&control,
        ContextControl::PendingRecovery { commands }
            if commands.len() == 2
                && commands.iter().all(|command| command.kind == PendingCommandKind::Action)
    ));
    assert!(!app
        .pending
        .values()
        .any(|item| matches!(item.command.action, Action::Send { .. })));
    assert!(app.focus_context_control(control));
    let focused = screen(&mut app, 82, 20);
    assert!(focused.contains("Unconfirmed action"));
    assert!(focused.contains("Enter act"));
    assert!(focused.contains("Esc back"));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&app.modal,
        Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Pending)
    ));
}
