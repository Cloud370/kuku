use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::Error;
use kuku_client::{Action, HistoryPage, Input, QueuedInput, Receipt, RewindGroup, Timing};
use serde_json::json;

use super::{app, click, key, render, screen};
use crate::app::{KeyboardFocus, Modal};
use crate::config::StatusLineSettings;
use crate::contextual::{ContextControl, PendingCommandIdentity, PendingCommandKind};
use crate::jobs::{Job, Update};
use crate::menu::PickerKind;
use crate::statusline::StatusLine;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

fn load_snapshot(app: &mut crate::app::App, status: &str, with_input: bool) {
    let mut snapshot = super::snapshot(app, status, json!(null), json!(null));
    if with_input {
        snapshot.agent.pending_inputs = 1;
        snapshot.pending.push(QueuedInput {
            input: Input {
                source: kuku_client::InputSource::Host("http".into()),
                ..Input::new("queued-command", app.active.agent.clone(), "waiting")
            },
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

#[test]
fn answer_popup_separates_scrolling_question_focused_editor_and_actions() {
    let mut app = app();
    let text = "Which target?\nReview the proposed approach.\nConfirm each necessary check.\nKeep any existing local changes.\nDo not modify unrelated files.\nPlease choose the final verification.\nFinal check?";
    let request =
        json!({"id":"question-popup","agent":"main","turn":1,"tool_call_id":"ask","text":text});
    let waiting = super::snapshot(&app, "waiting", request.clone(), json!(null));
    let history = serde_json::from_value(json!({
        "id":42,"agent":"main","turn":1,"type":"question","request":request,"text":text
    }))
    .unwrap();
    app.update(Update::Loaded(
        app.selection(),
        waiting,
        HistoryPage {
            entries: vec![history],
            rewind_groups: vec![],
            next_before: None,
        },
    ));
    app.pane_mut().draft.insert("independent main draft");
    let pending = screen(&mut app, 82, 28);
    assert_eq!(1, pending.matches("Final check?").count(), "{pending}");
    assert!(pending.contains("Waiting · main · F2 Reply"), "{pending}");

    let mut terminal = Terminal::new(TestBackend::new(82, 28)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let point = (app.request_area.x, app.request_area.y);
    click(&mut app, buffer, point);
    assert!(
        matches!(&app.modal, Some(Modal::Prompt(prompt)) if matches!(&prompt.kind, crate::menu::PromptKind::Question { request_id, .. } if request_id == "question-popup"))
    );
    let wide = screen(&mut app, 82, 28);
    for marker in [
        "Question · main",
        "Which target?",
        "Final check?",
        "Your reply",
        "Enter reply",
        "Esc back",
    ] {
        assert!(wide.contains(marker), "{marker} missing in\n{wide}");
    }

    let mut terminal = Terminal::new(TestBackend::new(26, 15)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let label = super::text_position(buffer, "Your reply");
    assert_eq!(buffer[label].bg, crate::theme::focused_row().bg.unwrap());
    let cursor = terminal.get_cursor_position().unwrap();
    assert_eq!(cursor.y, label.1 + 1);
    let narrow = screen(&mut app, 26, 15);
    for marker in [
        "Which target?",
        "Your reply",
        "Enter reply",
        "Esc back",
        "PgUp/PgDn question",
    ] {
        assert!(narrow.contains(marker), "{marker} missing in\n{narrow}");
    }
    for _ in 0..8 {
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        let view = screen(&mut app, 26, 15);
        if view.contains("Final check?") {
            break;
        }
    }
    let end = screen(&mut app, 26, 15);
    assert!(
        end.contains("Final check?") && end.contains("Your reply"),
        "{end}"
    );
    assert!(
        end.contains("Enter reply") && end.contains("Esc back"),
        "{end}"
    );

    key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!("independent main draft", app.pane().draft.text());
    assert!(app.composer_focused());
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let point = (app.request_area.x, app.request_area.y);
    click(&mut app, buffer, point);
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt)) if prompt.editor.text() == "a"));
    let restored = screen(&mut app, 26, 15);
    assert!(
        restored.contains("Which target?") && restored.contains("Your reply"),
        "{restored}"
    );
    app.effects.clear();
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.effects.iter().any(|effect| matches!(effect,
        Job::Save(command) if matches!(&command.action, Action::ReplyQuestion { request_id, text }
            if request_id == "question-popup" && text == "a")
    )));
    assert_eq!("independent main draft", app.pane().draft.text());
}

#[test]
fn history_confirmation_previews_only_verified_file_effects() {
    let mut app = app();
    let prompt = "Inspect the import and keep the original evidence for the next report.";
    app.review_history_jump(
        serde_json::from_value(json!({
            "event_id": 80, "can_continue": true, "turn": 41, "number": 42,
            "preview": prompt, "current_branch": true, "prompt_event_id": 50,
        }))
        .unwrap(),
    );
    let (selection, ticket) = app
        .effects
        .iter()
        .find_map(|job| match job {
            Job::HistoryJumpDraft(selection, ticket, 50) => Some((selection.clone(), *ticket)),
            _ => None,
        })
        .unwrap();
    app.update(Update::HistoryJumpDraft(
        selection.clone(),
        ticket,
        Ok(kuku_client::MessageDraft {
            text: prompt.into(),
            images: vec![],
        }),
    ));

    for (width, height) in [(100, 32), (60, 15)] {
        let rendered = screen(&mut app, width, height);
        assert!(rendered.contains("Switch to Turn 42"));
        assert!(rendered.contains("[ ] Restore files changed after this turn"));
        assert!(rendered.contains("write_file and edit_file"));
        assert!(rendered.contains("Script and shell changes"));
        assert!(rendered.contains("Enter Switch"));
        assert_eq!(1, rendered.matches("Inspect the import").count());
    }
    key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    let ticket = app
        .effects
        .iter()
        .find_map(|job| match job {
            Job::FileRewindPreview(_, ticket, 80) => Some(*ticket),
            _ => None,
        })
        .unwrap();
    app.update(Update::FileRewindPreview(
        selection,
        ticket,
        Ok(kuku_client::FileRewindPreview {
            fingerprint: "approved".into(),
            changes: vec![kuku_client::FileRewindChange {
                path: "report.txt".into(),
                remove: false,
                lines: Some((1, 2)),
            }],
            skipped: vec!["external.txt".into()],
        }),
    ));
    let rendered = screen(&mut app, 100, 32);
    assert!(rendered.contains("[x] Restore files changed after this turn"));
    assert!(rendered.contains("report.txt"));
    assert!(rendered.contains("Restore   +1 −2"));
    assert!(rendered.contains("external.txt"));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.effects.iter().any(|job| matches!(job, Job::Save(command) if matches!(
        &command.action, Action::HistoryJump { target_event_id: 80, file_restore: Some(value) } if value == "approved"
    ))));
}

#[test]
fn history_confirmation_with_no_restorable_files_still_switches() {
    let mut app = app();
    app.review_history_jump(
        serde_json::from_value(json!({
            "event_id": 80, "can_continue": true, "turn": 41, "number": 42,
            "preview": "Earlier message", "current_branch": true,
        }))
        .unwrap(),
    );
    key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    let (selection, ticket) = app
        .effects
        .iter()
        .find_map(|job| match job {
            Job::FileRewindPreview(selection, ticket, 80) => Some((selection.clone(), *ticket)),
            _ => None,
        })
        .unwrap();
    app.update(Update::FileRewindPreview(
        selection,
        ticket,
        Ok(kuku_client::FileRewindPreview {
            fingerprint: "empty-plan".into(),
            changes: Vec::new(),
            skipped: vec!["external.txt".into()],
        }),
    ));
    let rendered = screen(&mut app, 100, 32);
    assert!(rendered.contains("[ ] Restore files changed after this turn"));
    assert!(rendered.contains("No files to restore"));
    assert!(rendered.contains("external.txt"));
    assert!(rendered.contains("Enter Switch"));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.modal.is_none());
    assert!(
        app.effects
            .iter()
            .any(|job| matches!(job, Job::Save(command) if matches!(
                &command.action, Action::HistoryJump { target_event_id: 80, file_restore: None }
            )))
    );
}

#[test]
fn focused_rewind_hint_uses_the_narrow_footer() {
    let mut app = super::app();
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
    app.pane_mut()
        .transcript
        .toggle_rewind_group(50)
        .expect("group expansion request");
    app.pane_mut().transcript.set_focused_rewind_group(Some(50));

    let width = 36;
    let hint = crate::ui::contextual::focused_hint(&app, width).expect("rewind hint");
    let lines: Vec<_> = hint
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect();
    assert!(lines[0].contains("Rewound · read-only"));
    assert!(lines[1].contains("browse") && lines[1].contains("Esc"));
    assert!(
        lines
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= width)
    );
    for columns in [32, 33] {
        let view = screen(&mut app, columns, 20);
        assert!(view.contains("Esc input"), "{columns} columns:\n{view}");
    }
}

#[test]
fn continue_confirmation_is_transient_and_does_not_replace_an_error() {
    let mut app = app();
    for preserve_error in [false, true] {
        if preserve_error {
            app.problem("Persistent request error");
        }
        app.issue(app.active.clone(), Action::Advance);
        let command = super::saved_input(&app);
        app.update(Update::Sent(
            command.command_id.clone(),
            Ok(super::queued_receipt(&command, 80)),
        ));
        let notice = app.notice.as_ref().expect("continue feedback");
        if preserve_error {
            assert_eq!("Persistent request error", notice.text);
            assert!(notice.error && !notice.transient());
        } else {
            assert_eq!("Continue request received", notice.text);
            assert!(notice.transient() && notice.deadline().is_some());
            key(&mut app, KeyCode::Left, KeyModifiers::NONE);
            assert!(app.notice.is_none());
        }
    }
}

#[test]
fn contextual_escape_hints_remain_complete_at_32_and_33_columns() {
    for status in ["running", "idle"] {
        let mut app = app();
        load_snapshot(&mut app, status, false);
        let control = app
            .contextual_controls()
            .into_iter()
            .find(|control| {
                if status == "running" {
                    matches!(control, ContextControl::Interrupt { .. })
                } else {
                    matches!(control, ContextControl::Messages)
                }
            })
            .unwrap();
        app.focus_context_control(control);
        for columns in [32, 33] {
            let view = screen(&mut app, columns, 20);
            assert!(view.contains("Esc back"), "{columns} columns:\n{view}");
            assert!(app.pane().contextual_focus.active);
        }
    }
}

#[test]
fn continue_receipt_for_a_previous_agent_keeps_the_current_feedback() {
    let mut app = app();
    app.issue(app.active.clone(), Action::Advance);
    let command = super::saved_input(&app);
    app.select(
        crate::app::Address::new(app.active.session.clone(), "reviewer".into()),
        true,
        "Reviewer".into(),
    );
    app.confirmation("Current Agent feedback");
    app.update(Update::Sent(
        command.command_id.clone(),
        Ok(super::queued_receipt(&command, 80)),
    ));
    assert_eq!("Current Agent feedback", app.notice.as_ref().unwrap().text);
}

fn pending_send(app: &mut crate::app::App) -> kuku_client::Command {
    app.pane_mut().exists = true;
    app.pane_mut().draft.set_text("pending send".into());
    app.send(Timing::Steer);
    let Some(Job::Save(command)) = app.effects.pop() else {
        panic!("send command should be saved")
    };
    app.update(Update::Saved(command.clone(), Ok(())));
    app.update(Update::Sent(
        command.command_id.clone(),
        Err(Error::Io(std::io::Error::other("receipt lost"))),
    ));
    command
}

#[test]
fn clicking_reconnect_only_focuses_it_and_enter_preserves_the_draft() {
    let mut app = app();
    app.connection_problem = Some("kuku service offline".into());
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
        .find(|hit| matches!(hit.control, ContextControl::Reconnect { .. }))
        .unwrap()
        .area;

    click(&mut app, buffer, (hit.x, hit.y));
    assert!(app.pane().contextual_focus.active);
    assert!(!app.restarting);
    assert_eq!("unsent draft", app.pane().draft.text());

    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.connecting);
    assert!(!app.restarting);
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
        Some(ContextControl::Messages)
    ));
}

#[test]
fn narrow_layout_keeps_contextual_service_action_keyboard_reachable() {
    let mut app = app();
    app.connection_problem = Some("kuku service offline".into());
    app.pane_mut().exists = true;
    let view = screen(&mut app, 24, 12);
    assert!(!view.is_empty());
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.pane().contextual_focus.active);
    let focused = screen(&mut app, 24, 12);
    assert!(focused.contains("Reconnect"));
    assert!(focused.contains("Enter act"));
    assert!(focused.contains("Esc back"));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.connecting);
    assert!(!app.restarting);
}

#[test]
fn focused_control_hints_render_with_default_and_custom_statuslines() {
    let mut ordinary = app();
    ordinary.pane_mut().exists = true;
    let ordinary_view = screen(&mut ordinary, 82, 20);
    assert!(!ordinary_view.contains("Restart"));
    assert!(!ordinary_view.contains("Connected"));

    let mut desktop = app();
    desktop.pane_mut().exists = true;
    key(&mut desktop, KeyCode::F(2), KeyModifiers::NONE);
    let default_view = screen(&mut desktop, 82, 20);
    assert!(default_view.contains("Messages"));
    assert!(default_view.contains("Enter act"));
    assert!(default_view.contains("Esc back"));

    let mut running = app();
    load_snapshot(&mut running, "running", false);
    key(&mut running, KeyCode::F(2), KeyModifiers::NONE);
    let running_view = screen(&mut running, 82, 20);
    assert!(running_view.contains("Esc back"));

    let mut custom = app();
    custom.pane_mut().exists = true;
    custom.statusline_settings = StatusLineSettings::in_memory(StatusLine::parse("input").unwrap());
    key(&mut custom, KeyCode::F(2), KeyModifiers::NONE);
    let custom_view = screen(&mut custom, 82, 20);
    assert!(custom_view.contains("Messages"));
    assert!(custom_view.contains("Enter act"));
    assert!(custom_view.contains("Esc back"));
}

#[test]
fn continue_requires_pending_inputs_for_idle_and_stopped_but_not_interrupted_work() {
    let mut idle = app();
    load_snapshot(&mut idle, "idle", true);
    assert!(
        idle.contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut idle_empty = app();
    load_snapshot(&mut idle_empty, "idle", false);
    assert!(
        !idle_empty
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut idle_without_count = app();
    load_snapshot(&mut idle_without_count, "idle", true);
    idle_without_count
        .pane_mut()
        .snapshot
        .as_mut()
        .unwrap()
        .agent
        .pending_inputs = 0;
    assert!(
        !idle_without_count
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut idle_without_queue = app();
    load_snapshot(&mut idle_without_queue, "idle", false);
    idle_without_queue
        .pane_mut()
        .snapshot
        .as_mut()
        .unwrap()
        .agent
        .pending_inputs = 1;
    assert!(
        !idle_without_queue
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut stopped = app();
    load_snapshot(&mut stopped, "stopped", true);
    assert!(
        stopped
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut no_input = app();
    load_snapshot(&mut no_input, "stopped", false);
    assert!(
        !no_input
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut interrupted = app();
    load_snapshot(&mut interrupted, "interrupted", false);
    interrupted
        .pane_mut()
        .snapshot
        .as_mut()
        .unwrap()
        .agent
        .last_error = Some("provider returned HTTP 400".into());
    assert!(
        interrupted
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut running = app();
    load_snapshot(&mut running, "running", true);
    assert!(
        !running
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );
    assert!(
        running
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Interrupt { .. }))
    );

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
    assert!(
        !cleaning
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );
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
fn idle_continue_focuses_advances_and_revalidates_queued_identity() {
    let mut idle = app();
    load_snapshot(&mut idle, "idle", true);
    idle.pane_mut()
        .draft
        .set_text("unsent composer draft".into());
    key(&mut idle, KeyCode::F(2), KeyModifiers::NONE);
    assert!(matches!(
        idle.pane().contextual_focus.cursor.as_ref(),
        Some(ContextControl::Continue { inputs, .. })
            if inputs.len() == 1 && inputs[0] == "queued-command"
    ));
    key(&mut idle, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!("unsent composer draft", idle.pane().draft.text());
    assert!(idle.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Advance)
    )));

    let mut cleaning = app();
    load_snapshot(&mut cleaning, "idle", true);
    cleaning.pane_mut().snapshot.as_mut().unwrap().cleaning_up = true;
    assert!(
        !cleaning
            .contextual_controls()
            .iter()
            .any(|control| matches!(control, ContextControl::Continue { .. }))
    );

    let mut stale = app();
    load_snapshot(&mut stale, "idle", true);
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
fn focused_escape_returns_to_composer_before_interrupting_a_running_task() {
    let mut app = app();
    load_snapshot(&mut app, "running", true);
    app.pane_mut().draft.insert("draft not submitted");
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.pane().contextual_focus.active);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.pane().contextual_focus.active);
    assert!(app.effects.is_empty());
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Interrupt { execution_id: 49 })
    )));
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    assert_eq!("draft not submitted", app.pane().draft.text());
    assert_eq!(1, app.pane().deliveries.rows().len());
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
    for action in [
        Action::Configure {
            config: Box::default(),
        },
        Action::CreateAgent {
            config: Box::default(),
        },
    ] {
        app.issue(app.active.clone(), action);
        let Some(Job::Save(command)) = app.effects.pop() else {
            panic!("saved action should be queued")
        };
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
    assert!(
        !app.pending
            .values()
            .any(|item| matches!(item.command.action, Action::Send { .. }))
    );
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
