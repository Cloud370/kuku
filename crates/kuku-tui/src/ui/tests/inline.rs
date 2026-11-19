use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::{
    Acknowledgement, Action, HistoryPage, Input, InputSource, InputStatus, InputTiming,
    QueuedInput, Receipt,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

use super::*;
use crate::app::KeyboardFocus;
use crate::jobs::Update;
use crate::menu::PromptKind;

fn queued_state(app: &App, count: usize) -> Box<kuku_client::Snapshot> {
    let mut state = snapshot(app, "running", json!(null), json!(null));
    for index in 0..count {
        let id = format!("queue-{index}");
        let mut input = Input::new(id.clone(), "main", format!("queued text {index}"));
        input.source = InputSource::Host("http".into());
        input.timing = if index % 2 == 0 {
            InputTiming::NextTurn
        } else {
            InputTiming::NextStep
        };
        state.pending.push(QueuedInput {
            input,
            receipt: Receipt {
                command_id: id,
                event_id: 60 + index as u64,
                agent: "main".into(),
            },
        });
    }
    state
}

fn load(app: &mut App, state: Box<kuku_client::Snapshot>) {
    app.update(Update::Loaded(
        app.selection(),
        state,
        HistoryPage {
            entries: vec![],
            next_before: None,
        },
    ));
}

fn begin_withdrawal(app: &mut App) -> kuku_client::Command {
    let state = queued_state(app, 1);
    load(app, state);
    app.effects.clear();
    app.focus_queue_input("queue-0".into());
    key(app, KeyCode::Delete, KeyModifiers::NONE);
    saved_input(app)
}

fn acknowledge_withdrawal(
    app: &mut App,
    command: &kuku_client::Command,
    status: Option<InputStatus>,
) {
    app.update(Update::Sent(
        command.command_id.clone(),
        Ok(Acknowledgement {
            receipt: Receipt {
                command_id: command.command_id.clone(),
                event_id: 101,
                agent: command.target.agent.clone(),
            },
            replayed: false,
            status,
            warnings: vec![],
        }),
    ));
}

#[test]
fn inline_queue_click_scroll_and_edit_keep_the_selected_identity() {
    let mut app = app();
    let mut state = queued_state(&app, 8);
    let mut host_input = Input::new("host-only", "main", "internal host input");
    host_input.source = InputSource::Host("tool".into());
    state.pending.insert(
        0,
        QueuedInput {
            input: host_input,
            receipt: Receipt {
                command_id: "host-only".into(),
                event_id: 99,
                agent: "main".into(),
            },
        },
    );
    load(&mut app, state);
    app.effects.clear();
    app.pane_mut().draft.insert("independent draft");
    let mut terminal = Terminal::new(TestBackend::new(92, 26)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let (visible_row, selected_id) = app
        .delivery_hits
        .first()
        .map(|(range, id)| (range.start, id.clone()))
        .expect("visible ordinary queued row");
    assert_ne!("host-only", selected_id);
    let point = (app.delivery_area.x, app.delivery_area.y + visible_row);
    click(&mut app, buffer, point);
    assert_eq!(
        Some(selected_id.as_str()),
        app.pane().selected_queue_input.as_deref()
    );
    assert_eq!(KeyboardFocus::Queue, app.pane().keyboard_focus);
    assert_eq!("independent draft", app.pane().draft.text());
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    assert!(app.effects.iter().any(|job| matches!(job,
        crate::jobs::Job::Save(command)
            if matches!(command.action, Action::Cancel { execution_id: 49 })
    )));
    app.effects.clear();
    app.focus_queue_input(selected_id);

    key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(Some("queue-2"), app.pane().selected_queue_input.as_deref());
    assert!(screen(&mut app, 92, 26).contains("queued text 2"));
    screen(&mut app, 34, 18);
    assert!(app
        .delivery_hits
        .iter()
        .any(|(_, input_id)| input_id == "queue-2"));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::QueuedInput { input_command_id } if input_command_id == "queue-2")
            && prompt.editor.text() == "queued text 2"));
    key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    event(&mut app, Event::Paste("edited queue item".into()));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&saved_input(&app).action,
        Action::ReplaceQueued { input_command_id, text }
            if input_command_id == "queue-2" && text == "edited queue item"));
    assert_eq!("independent draft", app.pane().draft.text());
}

#[test]
fn empty_composer_up_selects_and_delete_withdraws_the_exact_queue_entry() {
    let mut app = app();
    let state = queued_state(&app, 3);
    load(&mut app, state);
    key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(KeyboardFocus::Queue, app.pane().keyboard_focus);
    assert_eq!(Some("queue-2"), app.pane().selected_queue_input.as_deref());
    key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(Some("queue-1"), app.pane().selected_queue_input.as_deref());
    key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
    assert!(matches!(&saved_input(&app).action,
        Action::Withdraw { input_command_id } if input_command_id == "queue-1"));
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
}

#[test]
fn confirmed_withdrawal_restores_text_to_its_original_pane_after_snapshot_removal() {
    let mut app = app();
    let command = begin_withdrawal(&mut app);
    let origin = app.active.clone();
    let removed = snapshot(&app, "running", json!(null), json!(null));
    load(&mut app, removed);

    let other = crate::app::Address::new(format!("{}-other", origin.session), origin.agent.clone());
    app.select(other, true, "Other session".into());
    acknowledge_withdrawal(&mut app, &command, Some(InputStatus::Withdrawn));
    assert!(app.pane().draft.text().is_empty());

    app.select(origin, true, "Main".into());
    assert_eq!("queued text 0", app.pane().draft.text());
}

#[test]
fn withdrawal_never_overwrites_a_draft_or_restores_an_unwithdrawn_input() {
    let mut typed = app();
    let command = begin_withdrawal(&mut typed);
    typed.pane_mut().draft.insert("typed while waiting");
    acknowledge_withdrawal(&mut typed, &command, Some(InputStatus::Withdrawn));
    assert_eq!("typed while waiting", typed.pane().draft.text());

    for status in [InputStatus::Accepted, InputStatus::Queued] {
        let mut app = app();
        let command = begin_withdrawal(&mut app);
        acknowledge_withdrawal(&mut app, &command, Some(status));
        assert!(app.pane().draft.text().is_empty());
    }

    let mut errored = app();
    let command = begin_withdrawal(&mut errored);
    errored.update(Update::Sent(
        command.command_id.clone(),
        Err(kuku_client::Error::Protocol("test failure".into())),
    ));
    assert!(errored.pane().draft.text().is_empty());

    let mut existing = app();
    existing.pane_mut().draft.insert("preexisting draft");
    let command = begin_withdrawal(&mut existing);
    acknowledge_withdrawal(&mut existing, &command, Some(InputStatus::Withdrawn));
    assert_eq!("preexisting draft", existing.pane().draft.text());
}

#[test]
fn stale_queue_selection_never_redirects_withdrawal_to_another_input() {
    let mut app = app();
    let state = queued_state(&app, 3);
    load(&mut app, state);
    app.focus_queue_input("queue-1".into());
    app.pane_mut()
        .snapshot
        .as_mut()
        .unwrap()
        .pending
        .retain(|entry| entry.receipt.command_id != "queue-1");
    app.effects.clear();

    let view = screen(&mut app, 72, 18);
    assert!(view.contains("No longer queued"));
    key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
    assert!(!app.effects.iter().any(|effect| matches!(effect,
        crate::jobs::Job::Save(command)
            if matches!(&command.action, Action::Withdraw { .. } | Action::ReplaceQueued { .. }))));
}

#[test]
fn pending_queue_mutations_block_duplicate_edits_and_withdrawals() {
    let mut app = app();
    let state = queued_state(&app, 2);
    load(&mut app, state);
    app.focus_queue_input("queue-1".into());
    app.issue(
        app.active.clone(),
        Action::ReplaceQueued {
            input_command_id: "queue-1".into(),
            text: "first edit".into(),
        },
    );
    assert!(app.queue_mutation_pending("queue-1"));
    app.effects.clear();
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.modal.is_none());
    key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
    assert!(!app.effects.iter().any(|effect| matches!(effect,
        crate::jobs::Job::Save(command)
            if matches!(&command.action, Action::Withdraw { .. } | Action::ReplaceQueued { .. }))));

    let first_session = app.active.clone();
    let other_session = crate::app::Address::new(
        format!("{}-other", first_session.session),
        first_session.agent,
    );
    app.select(other_session, true, "Other session".into());
    let second_session_state = queued_state(&app, 2);
    load(&mut app, second_session_state);
    app.focus_queue_input("queue-1".into());
    assert!(!app.queue_mutation_pending("queue-1"));
    app.edit_queued("queue-1".into());
    assert!(matches!(app.modal, Some(Modal::Prompt(_))));
}

#[test]
fn request_card_is_visible_without_focus_and_question_answer_is_scoped_and_stale_safe() {
    let mut app = app();
    let question = json!({
        "id":"q-1", "agent":"main", "turn":1,
        "tool_call_id":"tool-1", "text":"Which test target?"
    });
    let state = snapshot(&app, "waiting", question.clone(), json!(null));
    load(&mut app, state);
    app.effects.clear();
    app.pane_mut().draft.insert("my independent draft");
    let view = screen(&mut app, 82, 20);
    assert!(view.contains("Question") && view.contains("Which test target?"));
    assert!(app.composer_focused());
    let narrow = screen(&mut app, 24, 12);
    assert!(narrow.contains("Question"));

    let mut terminal = Terminal::new(TestBackend::new(82, 20)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let point = (app.request_area.x, app.request_area.y);
    click(&mut app, buffer, point);
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::Question { request_id, .. } if request_id == "q-1")));
    if let Some(Modal::Prompt(prompt)) = &mut app.modal {
        prompt.editor.set_text("answer draft".into());
    }
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    assert_eq!("my independent draft", app.pane().draft.text());
    app.effects.clear();
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(app.modal.is_none());
    assert!(matches!(
        app.pane().contextual_focus.cursor,
        Some(crate::contextual::ContextControl::Restart { .. })
    ));
    for _ in 0..5 {
        if matches!(&app.modal, Some(Modal::Prompt(prompt))
            if matches!(&prompt.kind, PromptKind::Question { request_id, .. } if request_id == "q-1"))
        {
            break;
        }
        if matches!(&app.modal, Some(Modal::Picker(picker))
            if matches!(picker.kind, PickerKind::Messages))
        {
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        } else {
            key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
        }
    }
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::Question { request_id, .. } if request_id == "q-1")
            && prompt.editor.text() == "answer draft"));
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if prompt.editor.text() == "answer draft"));
    if let Some(Modal::Prompt(prompt)) = &mut app.modal {
        assert_eq!("answer draft", prompt.editor.text());
    }
    let replacement_question = json!({
        "id":"q-2", "agent":"main", "turn":2,
        "tool_call_id":"tool-2", "text":"What should run instead?"
    });
    let stale_state = snapshot(&app, "waiting", replacement_question, json!(null));
    app.pane_mut().snapshot = Some(stale_state);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.effects.is_empty());
    assert!(app.modal.is_none());
    assert_eq!("my independent draft", app.pane().draft.text());
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
}

#[test]
fn reconnect_shortcut_survives_permission_request_focus_without_answering_it() {
    let mut app = app();
    let permission = json!({
        "id":"p-reconnect", "conversation":"main", "turn":1,
        "tool_call_id":"tool-reconnect", "tool":"write_file", "risk":"high",
        "summary":"Change app.rs", "candidate":"diff", "source":"tool"
    });
    let permission_state = snapshot(&app, "waiting", json!(null), permission);
    load(&mut app, permission_state);
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert_eq!(KeyboardFocus::Request, app.pane().keyboard_focus);
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(matches!(
        app.pane().contextual_focus.cursor,
        Some(crate::contextual::ContextControl::Restart { .. })
    ));
    key(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(KeyboardFocus::Request, app.pane().keyboard_focus);
    app.effects.clear();
    key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app
        .effects
        .iter()
        .any(|effect| matches!(effect, crate::jobs::Job::Connect)));
    assert!(!app.effects.iter().any(|effect| matches!(effect,
        crate::jobs::Job::Save(command)
            if matches!(command.action, Action::ReplyPermission { .. })
    )));
}

#[test]
fn question_reply_restores_composer_after_request_clears() {
    let mut app = app();
    let question = json!({
        "id":"q-submit", "agent":"main", "turn":1,
        "tool_call_id":"tool-submit", "text":"What should run?"
    });
    let waiting = snapshot(&app, "waiting", question, json!(null));
    load(&mut app, waiting);
    app.effects.clear();
    app.pane_mut().draft.insert("existing draft");
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    if let Some(Modal::Prompt(prompt)) = &mut app.modal {
        prompt.editor.set_text("run workspace tests".into());
    }
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    assert!(app.effects.iter().any(|effect| matches!(effect,
        crate::jobs::Job::Save(command)
            if matches!(&command.action, Action::ReplyQuestion { request_id, text }
                if request_id == "q-submit" && text == "run workspace tests"))));

    let idle = snapshot(&app, "idle", json!(null), json!(null));
    load(&mut app, idle);
    key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!("existing draftx", app.pane().draft.text());
}

#[test]
fn question_reply_is_rejected_after_switching_panes() {
    let mut app = app();
    let question = json!({
        "id":"q-1", "agent":"main", "turn":1,
        "tool_call_id":"tool-1", "text":"Which test target?"
    });
    let state = snapshot(&app, "waiting", question, json!(null));
    load(&mut app, state);
    app.pane_mut().draft.insert("keep this draft");
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    if let Some(Modal::Prompt(prompt)) = &mut app.modal {
        prompt.editor.set_text("workspace tests".into());
    }
    let main = app.active.clone();
    let reviewer = crate::app::Address::new(app.active.session.clone(), "reviewer".into());
    app.select(reviewer, true, "Reviewer".into());
    app.effects.clear();
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!app.effects.iter().any(|effect| matches!(effect,
        crate::jobs::Job::Save(command)
            if matches!(&command.action, Action::ReplyQuestion { .. }))));
    assert!(app.modal.is_none());
    app.select(main, true, "Main".into());
    assert_eq!("keep this draft", app.pane().draft.text());
}

#[test]
fn permission_card_requires_an_explicit_choice_and_stale_requests_do_not_reply() {
    let mut app = app();
    let permission = json!({
        "id":"p-1", "conversation":"main", "turn":1, "tool_call_id":"tool-1",
        "tool":"write_file", "risk":"high", "summary":"Change app.rs",
        "candidate":"diff", "source":"tool"
    });
    let state = snapshot(&app, "waiting", json!(null), permission.clone());
    load(&mut app, state);
    app.effects.clear();
    app.pane_mut().draft.insert("keep this draft");
    let view = screen(&mut app, 84, 22);
    assert!(view.contains("Permission") && view.contains("Change app.rs"));
    assert!(view.contains("F2 focus") && view.contains("Enter decide"));
    assert!(view.contains("Deny") && view.contains("Allow once"));
    assert!(app.composer_focused());

    key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
    assert_eq!("keep this drafty", app.pane().draft.text());
    assert!(app.effects.is_empty());
    let mut terminal = Terminal::new(TestBackend::new(84, 22)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let allow = app.permission_choice_areas[1];
    let point = (allow.x, allow.y);
    click(&mut app, buffer, point);
    assert_eq!(Some(true), app.pane().permission_choice);
    assert!(app.effects.is_empty());
    let details = screen(&mut app, 84, 32);
    assert!(details.contains("Candidate:"), "{details:?}");
    assert!(details.contains("diff"), "{details:?}");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&saved_input(&app).action,
        Action::ReplyPermission { request_id, allow: true } if request_id == "p-1"));
    assert_eq!("keep this drafty", app.pane().draft.text());

    app.effects.clear();
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    assert!(app.effects.is_empty());

    let mut deny = super::app();
    let mut waiting = super::app();
    let waiting_state = snapshot(&waiting, "waiting", json!(null), permission.clone());
    load(&mut waiting, waiting_state);
    waiting.effects.clear();
    key(&mut waiting, KeyCode::Esc, KeyModifiers::NONE);
    assert!(waiting.effects.is_empty());
    assert!(waiting.escape_armed_at.is_none());

    let deny_state = snapshot(&deny, "waiting", json!(null), permission.clone());
    load(&mut deny, deny_state);
    key(&mut deny, KeyCode::F(2), KeyModifiers::NONE);
    key(&mut deny, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(Some(false), deny.pane().permission_choice);
    key(&mut deny, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&saved_input(&deny).action,
        Action::ReplyPermission { request_id, allow: false } if request_id == "p-1"));

    let mut stale = super::app();
    let state = snapshot(&stale, "waiting", json!(null), permission);
    load(&mut stale, state);
    key(&mut stale, KeyCode::F(2), KeyModifiers::NONE);
    let no_request = snapshot(&stale, "waiting", json!(null), json!(null));
    stale.pane_mut().snapshot = Some(no_request);
    stale.effects.clear();
    key(&mut stale, KeyCode::Right, KeyModifiers::NONE);
    key(&mut stale, KeyCode::Enter, KeyModifiers::NONE);
    assert!(stale.effects.is_empty());
    assert_eq!(KeyboardFocus::Composer, stale.pane().keyboard_focus);
}

#[test]
fn short_permission_details_scroll_reaches_the_end_and_clamps() {
    let mut app = app();
    let permission = json!({
        "id":"p-scroll", "conversation":"main", "turn":1, "tool_call_id":"tool-scroll",
        "tool":"write_file", "risk":"high", "summary":"Review long candidate",
        "candidate":format!("{}DETAIL-END", "x".repeat(240)), "source":"tool"
    });
    let state = snapshot(&app, "waiting", json!(null), permission);
    load(&mut app, state);
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    let mut view = screen(&mut app, 48, 12);
    let page = app.pane().request_detail_page_size;
    assert!(
        (1..=3).contains(&page),
        "unexpected details page size {page}"
    );
    for _ in 0..40 {
        if view.contains("DETAIL-END") {
            break;
        }
        let before = app.pane().request_scroll;
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        view = screen(&mut app, 48, 12);
        if before == app.pane().request_scroll {
            break;
        }
    }
    assert!(view.contains("DETAIL-END"), "{view}");
    let end = app.pane().request_scroll;
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(end, app.pane().request_scroll);
}

#[test]
fn stale_permission_click_cannot_focus_or_select_a_replacement_request() {
    let mut app = app();
    let permission = json!({
        "id":"p-1", "conversation":"main", "turn":1, "tool_call_id":"tool-1",
        "tool":"write_file", "risk":"high", "summary":"Change app.rs",
        "candidate":"diff", "source":"tool"
    });
    let state = snapshot(&app, "waiting", json!(null), permission);
    load(&mut app, state);
    let mut terminal = Terminal::new(TestBackend::new(84, 22)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let allow = app.permission_choice_areas[1];
    let point = (allow.x, allow.y);
    assert_eq!(
        Some("p-1"),
        app.request_hit.as_ref().map(|(_, id)| id.as_str())
    );

    let replacement = json!({
        "id":"p-2", "conversation":"main", "turn":1, "tool_call_id":"tool-2",
        "tool":"write_file", "risk":"high", "summary":"Change config.rs",
        "candidate":"diff", "source":"tool"
    });
    let replacement_state = snapshot(&app, "waiting", json!(null), replacement);
    app.pane_mut().snapshot = Some(replacement_state);
    app.effects.clear();
    click(&mut app, buffer, point);
    assert_eq!(KeyboardFocus::Composer, app.pane().keyboard_focus);
    assert_eq!(None, app.pane().permission_choice);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!app.effects.iter().any(|effect| matches!(effect,
        crate::jobs::Job::Save(command)
            if matches!(&command.action, Action::ReplyPermission { .. }))));
}

#[test]
fn help_overlay_is_inert_on_mouse_and_fits_narrow_terminals() {
    let mut empty = super::app();
    let hint = screen(&mut empty, 90, 24);
    assert!(hint.contains("Enter steer") && hint.contains("Alt+Enter next Turn"));
    assert!(hint.contains("/ actions") && hint.contains("Ctrl+P help"));

    let mut app = app();
    app.pane_mut().draft.insert("preserve me");
    let draft_view = screen(&mut app, 90, 24);
    assert!(!draft_view.contains("/ actions") && !draft_view.contains("Ctrl+P help"));
    key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
    let regular = screen(&mut app, 90, 24);
    assert!(regular.contains("Keyboard shortcuts") && regular.contains("Alt+Enter"));
    assert!(regular.contains("Close/back; stop active work"));
    assert!(!regular.contains("/queue"));
    let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    click(&mut app, buffer, (45, 8));
    assert!(matches!(&app.modal, Some(Modal::Help)));
    assert_eq!("preserve me", app.pane().draft.text());
    assert!(app.effects.is_empty());

    let narrow = screen(&mut app, 20, 8);
    assert!(!narrow.is_empty());
}
