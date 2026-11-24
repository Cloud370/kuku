use std::path::PathBuf;
mod activity;
mod contextual;
mod layout;
mod model_default;
mod reading;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use kuku_client::{
    Acknowledgement, Action, Command, Error, HistoryEntry, HistoryKind, HistoryPage, Input,
    InputSource, InputStatus, InputTiming, Observation, QueuedInput, Receipt, Snapshot, Target,
    Timing,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use super::{ActivityFrame, render, render_at};
use crate::app::{Address, App, KeyboardFocus, Modal};
use crate::jobs::{Job, Update};
use crate::menu::{Choice, ChoiceKind, MessageAction, PickerKind};
use crate::output::OutputViewer;
use crate::transcript::{MessageId, SelectedMessage, ToolView};

fn screen(app: &mut App, width: u16, height: u16) -> String {
    activity_screen(app, width, height, None).0
}

fn activity_screen(app: &mut App, width: u16, height: u16, step: Option<u128>) -> (String, bool) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut visible = false;
    let activity = ActivityFrame::test_frame(app, step);
    terminal
        .draw(|frame| visible = render_at(frame, app, activity))
        .unwrap();
    let text = buffer_rows(terminal.backend().buffer())
        .collect::<Vec<_>>()
        .join("\n");
    (text, visible)
}

fn buffer_rows(buffer: &Buffer) -> impl Iterator<Item = String> + '_ {
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
}

fn app() -> App {
    let workspace = PathBuf::from("/workspace");
    let mut app = App::new(workspace.clone(), None);
    app.update(Update::Connected {
        endpoint: "http://127.0.0.1:6789".into(),
        workspace,
    });
    app.complete_test_permissions();
    app.effects.clear();
    app
}

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    event(app, Event::Key(KeyEvent::new(code, modifiers)));
}

fn event(app: &mut App, event: Event) {
    app.event(event, &Buffer::empty(Rect::ZERO));
}

fn mouse(kind: MouseEventKind, point: (u16, u16), modifiers: KeyModifiers) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column: point.0,
        row: point.1,
        modifiers,
    })
}

fn click(app: &mut App, buffer: &Buffer, point: (u16, u16)) {
    let left = MouseButton::Left;
    app.event(
        mouse(MouseEventKind::Down(left), point, KeyModifiers::NONE),
        buffer,
    );
    app.event(
        mouse(MouseEventKind::Up(left), point, KeyModifiers::NONE),
        buffer,
    );
}

fn cell(buffer: &Buffer, symbol: &str) -> (u16, u16) {
    let index = buffer
        .content
        .iter()
        .position(|cell| cell.symbol() == symbol)
        .unwrap_or_else(|| panic!("missing screen symbol {symbol:?}"));
    buffer.pos_of(index)
}

fn text_position(buffer: &Buffer, text: &str) -> (u16, u16) {
    let area = buffer.area;
    for row in area.top()..area.bottom() {
        let line = (area.left()..area.right())
            .map(|column| buffer[(column, row)].symbol())
            .collect::<String>();
        if let Some(byte_index) = line.find(text) {
            let column = UnicodeWidthStr::width(&line[..byte_index]);
            return (area.left() + column as u16, row);
        }
    }
    panic!("missing rendered text {text:?}");
}

fn message_action(app: &mut App, id: u64, action: MessageAction) {
    app.open_messages_picker();
    let Some(Modal::Picker(picker)) = &mut app.modal else {
        panic!("message picker expected");
    };
    picker.selected = picker
        .visible_choices()
        .iter()
        .position(|choice| matches!(
            &choice.kind,
            ChoiceKind::MessageEntry(SelectedMessage { id: MessageId::Event(event), .. }) if *event == id
        ))
        .expect("message entry is available");
    key(app, KeyCode::Enter, KeyModifiers::NONE);
    let Some(Modal::Picker(picker)) = &mut app.modal else {
        panic!("message action picker expected");
    };
    picker.selected = picker
        .visible_choices()
        .iter()
        .position(|choice| {
            matches!(
                &choice.kind,
                ChoiceKind::MessageAction { action: candidate, .. } if *candidate == action
            )
        })
        .expect("message action is available");
    key(app, KeyCode::Enter, KeyModifiers::NONE);
}

fn entry(id: u64, agent: &str, kind: HistoryKind, text: &str) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: agent.into(),
        turn: Some(1),
        kind,
        text: Some(text.into()),
        turn_summary: None,
        details: Default::default(),
    }
}

fn snapshot(
    app: &App,
    status: &str,
    question: serde_json::Value,
    permission: serde_json::Value,
) -> Box<Snapshot> {
    Box::new(
        serde_json::from_value(json!({
            "session_id": app.active.session,
            "workspace": app.workspace,
            "cursor": 52,
            "pending": [],
            "agents": [],
            "selection": null,
            "agent": {
                "address": app.active.agent, "config": {}, "status": status, "pending_inputs": 0,
                "execution_id": 49, "question": question, "permission": permission,
                "usage": {"model_requests":0,"unreported_requests":0},
            }
        }))
        .unwrap(),
    )
}

fn saved_input(app: &App) -> Command {
    app.effects
        .iter()
        .rev()
        .find_map(|job| match job {
            Job::Save(command) => Some(command.clone()),
            _ => None,
        })
        .expect("queued command is saved before network delivery")
}

fn queued_receipt(command: &Command, event_id: u64) -> Acknowledgement {
    Acknowledgement {
        receipt: Receipt {
            command_id: command.command_id.clone(),
            event_id,
            agent: command.target.agent.clone(),
        },
        replayed: false,
        status: Some(InputStatus::Queued),
        warnings: vec![],
    }
}

#[test]
fn identical_pending_inputs_render_immediately_and_admission_wins_over_a_late_receipt() {
    let mut app = app();
    app.pane_mut().exists = true;
    app.pane_mut().title = "Concurrent inputs".into();
    let load = snapshot(&app, "running", json!(null), json!(null));
    app.update(Update::Loaded(
        app.selection(),
        load,
        HistoryPage {
            entries: vec![],
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    let text = "same pending text";

    app.pane_mut().draft.insert(text);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let first = saved_input(&app);
    let sending = screen(&mut app, 92, 25);
    assert!(sending.contains("You · Sending"));
    assert!(sending.find(text).unwrap() < sending.find("Enter steer").unwrap());
    app.update(Update::Saved(first.clone(), Ok(())));
    app.update(Update::Sent(
        first.command_id.clone(),
        Ok(queued_receipt(&first, 53)),
    ));
    assert!(screen(&mut app, 92, 25).contains("Queued · next step"));

    app.pane_mut().draft.insert(text);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let second = saved_input(&app);
    assert_ne!(first.command_id, second.command_id);
    assert_eq!(2, screen(&mut app, 92, 25).matches(text).count());
    app.update(Update::Saved(second.clone(), Ok(())));

    let active = app.selection();
    let committed =
        |value| Update::Observation(active.clone(), serde_json::from_value(value).unwrap());
    app.update(committed(json!({
        "type":"committed", "id":55, "agent":"main", "kind":"runtime.operation",
        "operation":"inputs_accepted", "delivery":{"status":"accepted","input_ids":[first.command_id, second.command_id]},
    })));
    app.update(committed(json!({
        "type":"committed", "id":56, "agent":"main", "kind":"message.user",
        "entry":{"id":56, "agent":"main", "type":"user", "turn":1, "turn_number":1, "text":format!("{text}\n\n{text}")},
    })));
    app.update(Update::Sent(
        second.command_id.clone(),
        Ok(queued_receipt(&second, 54)),
    ));
    let accepted = screen(&mut app, 92, 25);
    assert!(
        accepted
            .lines()
            .any(|line| line.contains("You") && line.contains("Turn 1"))
    );
    assert!(!accepted.contains("Pending inputs"));
    assert_eq!(2, accepted.matches(text).count());
}

#[test]
fn clicking_the_queue_focuses_its_inline_editor_and_saves_the_exact_replacement() {
    let mut app = app();
    let mut state = snapshot(&app, "running", json!(null), json!(null));
    let mut input = Input::new("original", "main", "old queued text\n次の行");
    input.source = InputSource::Host("http".into());
    input.timing = InputTiming::NextTurn;
    state.pending.push(QueuedInput {
        input,
        receipt: Receipt {
            command_id: "original".into(),
            event_id: 51,
            agent: "main".into(),
        },
    });
    app.update(Update::Loaded(
        app.selection(),
        state,
        HistoryPage {
            entries: vec![],
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.pane_mut().draft.insert("independent draft");
    let rendered = screen(&mut app, 92, 26);
    assert!(rendered.contains("Queued · next Turn") && rendered.contains("old queued text"));
    let mut terminal = Terminal::new(TestBackend::new(92, 26)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let visible_row = app
        .delivery_hits
        .first()
        .filter(|(_, id)| id == "original")
        .map(|(range, _)| range.start)
        .expect("visible queued input");
    let point = (app.delivery_area.x, app.delivery_area.y + visible_row);
    click(&mut app, buffer, point);
    assert!(app.modal.is_none());
    assert_eq!(KeyboardFocus::Queue, app.pane().keyboard_focus);
    assert_eq!(Some("original"), app.pane().selected_queue_input.as_deref());
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        matches!(&app.modal, Some(Modal::Prompt(prompt)) if prompt.editor.text() == "old queued text\n次の行")
    );
    key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    event(&mut app, Event::Paste("edit\r\n第三行".into()));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let replacement = saved_input(&app);
    assert!(
        matches!(&replacement.action, Action::ReplaceQueued { input_command_id, text, .. }
        if input_command_id == "original" && text == "edit\n第三行")
    );
    assert_eq!("independent draft", app.pane().draft.text());
    assert!(
        app.pane()
            .deliveries
            .rows()
            .iter()
            .any(|row| row.id == "original")
    );

    app.update(Update::Saved(replacement.clone(), Ok(())));
    app.update(Update::Sent(
        replacement.command_id.clone(),
        Ok(queued_receipt(&replacement, 53)),
    ));
    let rows = app.pane().deliveries.rows();
    assert_eq!(1, rows.len());
    assert_eq!(&replacement.command_id, &rows[0].id);
    assert_eq!(Timing::NextTurn, rows[0].timing);
    assert_eq!("edit\n第三行", rows[0].text);
}

#[test]
fn blank_welcome_scales_down_and_active_session_uses_one_branded_row() {
    let mut app = app();
    app.pane_mut().initial_config.tier = Some("work".into());
    app.startup_skills =
        crate::app::StartupSkills::Ready(vec!["review-workspace".into(), "format-code".into()]);
    for width in [64, 118] {
        let mut terminal = Terminal::new(TestBackend::new(width, 28)).unwrap();
        let frame = terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let rows: Vec<_> = buffer_rows(frame.buffer).collect();
        let x = usize::from(app.composer_area.x);
        assert_eq!("", rows[0].trim());
        for (row, text) in [
            (1, "⢎⡱  kuku · New session"),
            (3, "Workspace  /workspace"),
            (4, "Model      Tier work"),
            (6, "Skills   /skill: Browse"),
            (7, "  review-workspace · format-code"),
        ] {
            assert_eq!(format!("{}{text}", " ".repeat(x)), rows[row].trim_end());
            assert!(
                frame.buffer.content[row * usize::from(width)..(row + 1) * usize::from(width)]
                    .iter()
                    .all(|cell| cell.bg == ratatui::style::Color::Reset)
            );
        }
        assert!(
            rows[9]
                .trim_start()
                .starts_with("Enter sends your first prompt")
        );
        let shortcuts_row = 10 + usize::from(width == 64);
        assert!(rows[shortcuts_row - 1].contains("/resume opens a saved session"));
        assert_eq!(
            "/ Actions · Ctrl+L Model · Ctrl+T Think · Ctrl+P Help",
            rows[shortcuts_row].trim()
        );
    }

    let short = screen(&mut app, 29, 6);
    assert!(short.lines().next().unwrap().contains("kuku · New session"));
    assert!(short.contains("Enter to send · /resume"));

    app.select(app.active.clone(), true, "修复使用中文标题的会话".into());
    let active = screen(&mut app, 31, 8);
    let heading = active.lines().next().unwrap();
    assert!(heading.starts_with(" kuku · "));
    assert!(heading.contains("修") && heading.contains("复"));
    assert!(active.contains("Opening"));
    assert!(!active.contains("New session"));
}

#[test]
fn blank_welcome_shows_the_shortened_workspace_path() {
    let mut app = app();
    app.workspace = PathBuf::from(format!("/{}/actual-workspace", "long".repeat(40)));
    let welcome = screen(&mut app, 118, 28);
    let workspace = welcome
        .lines()
        .find(|line| line.trim_start().starts_with("Workspace "))
        .unwrap();
    assert!(workspace.contains('…'));
    assert!(workspace.trim_end().ends_with("actual-workspace"));
}

#[test]
fn blank_configuration_screen_keeps_pasted_draft_on_wide_and_narrow_terminals() {
    let mut app = App::new(PathBuf::from("/workspace"), None);
    app.update(Update::ConnectFailed(Error::Options(
        "Run kuku init to configure a model".into(),
    )));
    let first = screen(&mut app, 104, 24);
    assert!(first.contains("New session"));
    assert!(first.contains("kuku init"));
    event(
        &mut app,
        Event::Paste("中文\r\nfirst\rlast e\u{301}".into()),
    );
    key(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
    key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!("中文\nfirst\nlast e\u{301}\nx", app.pane().draft.text());
    let view = screen(&mut app, 29, 13);
    assert!(view.contains("last e\u{301}"));
    assert!(view.contains("Offline"));
}

#[test]
fn switching_agent_keeps_drafts_and_read_position_and_ignores_old_observer() {
    let mut app = app();
    let main = app.active.clone();
    app.select(main.clone(), true, "Example".into());
    let main_selection = app.selection();
    let page = HistoryPage {
        entries: (1..46)
            .map(|id| {
                entry(
                    id,
                    "main",
                    HistoryKind::Assistant,
                    &format!("main-reply-{id}"),
                )
            })
            .collect(),
        rewind_groups: Vec::new(),
        next_before: Some(1),
    };
    let mut main_state = snapshot(&app, "running", json!(null), json!(null));
    main_state.agents = serde_json::from_value(json!([
        {"address":"main", "status":"running", "pending_inputs":0},
        {"address":"reviewer", "status":"idle", "pending_inputs":0},
    ]))
    .unwrap();
    app.update(Update::Loaded(main_selection.clone(), main_state, page));
    event(&mut app, Event::Paste("main-draft".into()));
    screen(&mut app, 79, 22);
    key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    let before = screen(&mut app, 79, 22);
    let review = Address::new(main.session.clone(), "reviewer".into());
    app.select(review.clone(), true, "Example".into());
    event(&mut app, Event::Paste("review-notes".into()));
    app.update(Update::Observation(
        main_selection,
        Observation::Live {
            agent: "main".into(),
            execution_id: 49,
            cursor: 52,
            turn: Some(1),
            turn_number: Some(1),
            event: json!({"type":"text","content":"obsolete main stream"}),
        },
    ));
    let other = screen(&mut app, 79, 22);
    assert!(other.contains("Send to reviewer"));
    assert!(other.contains("review-notes"));
    assert!(!other.contains("obsolete main stream"));
    app.select(main, true, "Example".into());
    let after = screen(&mut app, 79, 22);
    assert_eq!(
        before.lines().skip(1).take(11).collect::<Vec<_>>(),
        after.lines().skip(1).take(11).collect::<Vec<_>>()
    );
    assert!(before.contains("Reading history · ↑↓ scroll"), "{before}");
    assert!(after.lines().next().unwrap().contains("/agents"));
    assert!(after.contains("main-draft"));
    app.select(review, true, "Example".into());
    key(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    let command = app
        .effects
        .iter()
        .find_map(|effect| {
            if let Job::Save(command) = effect {
                Some(command)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!("reviewer", command.target.agent);
    assert!(
        matches!(&command.action, Action::Send { text, timing: Timing::NextTurn, .. } if text == "review-notes")
    );
}

#[test]
fn source_action_completes_on_tab_and_toggles_one_pane_without_changing_copy() {
    let mut app = app();
    let main = app.active.clone();
    let reviewer = Address::new(main.session.clone(), "reviewer".into());
    let raw = "````markdown\n# Document\n\n```rust\nlet x = 1;\n```\n````";
    app.select(main.clone(), true, "Example".into());
    app.pane_mut()
        .transcript
        .apply(entry(1, "main", HistoryKind::Assistant, raw), None);
    app.pane_mut().draft.insert("main draft");
    app.select(reviewer.clone(), true, "Example".into());
    app.pane_mut().transcript.apply(
        entry(2, "reviewer", HistoryKind::Assistant, "**Reviewer reply**"),
        None,
    );
    app.pane_mut().draft.insert("reviewer draft");
    app.select(main.clone(), true, "Example".into());

    message_action(&mut app, 1, MessageAction::Source);
    let source = screen(&mut app, 100, 24);
    assert!(source.contains("Assistant · source"));
    assert!(source.contains("````markdown") && source.contains("```rust"));
    assert!(source.contains("main draft"));

    message_action(&mut app, 1, MessageAction::Copy);
    assert!(
        app.effects
            .iter()
            .any(|job| matches!(job, Job::Copy(text) if text == raw))
    );
    app.select(reviewer, true, "Example".into());
    let other = screen(&mut app, 100, 24);
    assert!(other.contains("Reviewer reply") && other.contains("reviewer draft"));
    assert!(!other.contains("**Reviewer reply**"));
    app.select(main, true, "Example".into());
    assert!(screen(&mut app, 100, 24).contains("````markdown"));
    message_action(&mut app, 1, MessageAction::Source);
    assert!(!screen(&mut app, 100, 24).contains("````markdown"));
}

#[test]
fn reading_arrows_and_clicks_keep_pending_input_drafts_and_live_content_scoped() {
    let mut app = app();
    let mut state = snapshot(&app, "running", json!(null), json!(null));
    let mut input = Input::new("queue-1", "main", "queued text");
    input.source = InputSource::Host("http".into());
    state.pending.push(QueuedInput {
        input,
        receipt: Receipt {
            command_id: "queue-1".into(),
            event_id: 30,
            agent: "main".into(),
        },
    });
    let entries = (1..30)
        .map(|id| {
            entry(
                id,
                "main",
                HistoryKind::Assistant,
                &format!("Checkpoint {id:02}"),
            )
        })
        .collect();
    app.update(Update::Loaded(
        app.selection(),
        state,
        HistoryPage {
            entries,
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.pane_mut().draft.remember("prior prompt");
    screen(&mut app, 105, 24);
    key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    let painted = screen(&mut app, 105, 24);
    assert!(painted.contains("Reading history · ↑↓ scroll") && painted.contains("Pending inputs"));
    let (width, height) = app.transcript_area;
    let before = app.pane_mut().transcript.project(width, height).lines;
    key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    let earlier = app.pane_mut().transcript.project(width, height).lines;
    assert_eq!(&before[..height - 1], &earlier[1..]);
    assert!(app.modal.is_none() && app.pane().draft.text().is_empty());
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        before,
        app.pane_mut().transcript.project(width, height).lines
    );

    app.pane_mut().transcript.live(
        49,
        Some(1),
        Some(1),
        &json!({"type":"text","content":"fresh output"}),
    );
    assert!(screen(&mut app, 105, 24).contains("New content"));
    assert_eq!(
        before,
        app.pane_mut().transcript.project(width, height).lines
    );
    assert!(screen(&mut app, 24, 12).contains("New · Alt+↑"));
    key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
    event(&mut app, Event::Paste("raft".into()));
    let editing_screen = screen(&mut app, 105, 24);
    assert!(
        editing_screen.contains("Reading history · Editing"),
        "{editing_screen}"
    );
    assert_eq!("draft", app.pane().draft.text());
    assert_eq!(
        before,
        app.pane_mut().transcript.project(width, height).lines
    );

    let mut terminal = Terminal::new(TestBackend::new(105, 24)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    app.event(
        mouse(MouseEventKind::ScrollUp, (4, 5), KeyModifiers::NONE),
        buffer,
    );
    assert!(!app.composer_focused());
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let composer = (app.composer_area.x, app.composer_area.y);
    click(&mut app, buffer, composer);
    assert!(app.composer_focused() && app.pane().transcript.is_reading());
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let jump = (app.reading_area.x, app.reading_area.y);
    click(&mut app, buffer, jump);
    let latest = screen(&mut app, 105, 24);
    assert!(latest.contains("fresh output") && !latest.contains("Reading history"));
    assert!(latest.contains("queued text"));
    assert_eq!("draft", app.pane().draft.text());
}

#[test]
fn waiting_request_is_explicit_and_escape_preserves_message_draft() {
    let mut app = app();
    app.select(app.active.clone(), true, "Waiting conversation".into());
    let target = app.selection();
    let waiting = snapshot(
        &app,
        "waiting",
        json!({
            "id":"q-1", "agent":"main", "turn":1,"tool_call_id":"tool-1","text":"Which test target?"
        }),
        json!(null),
    );
    app.update(Update::Loaded(
        target,
        waiting,
        HistoryPage {
            entries: vec![],
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    event(&mut app, Event::Paste("my independent draft".into()));
    let view = screen(&mut app, 82, 20);
    assert!(view.contains("Waiting · main · F2 Reply"));
    assert!(view.contains("Which test target?"));
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!("my independent draft", app.pane().draft.text());
    assert!(
        !app.effects
            .iter()
            .any(|effect| matches!(effect, Job::Save(_)))
    );
    let control_count = app.contextual_controls().len();
    for _ in 0..control_count {
        key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    }
    event(&mut app, Event::Paste("workspace tests".into()));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let action = app
        .effects
        .iter()
        .find_map(|effect| {
            if let Job::Save(command) = effect {
                Some(&command.action)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        matches!(action, Action::ReplyQuestion { request_id, text } if request_id == "q-1" && text == "workspace tests")
    );
    assert_eq!("my independent draft", app.pane().draft.text());
}

#[test]
fn generic_tool_details_render_control_bytes_visibly_and_copy_original_reply() {
    let mut app = app();
    app.select(app.active.clone(), true, "Tool test".into());
    let reply = "## Result\n\n```rust\nlet number = 4;\n```";
    let tool_call = serde_json::from_value(json!({
        "id":3,"agent":"main","turn":1,"type":"tool_call","tool":"example_custom","tool_call_id":"t-1","args":{}
    })).unwrap();
    let tool_result = serde_json::from_value(json!({
        "id":4,"agent":"main","turn":1,"type":"tool_result","tool_call_id":"t-1","status":"ok","summary":"finished",
        "model_content":"before\u{001b}[2Jafter","output_id":"saved-output","structured":{"exit_code":0}
    })).unwrap();
    let page = HistoryPage {
        entries: vec![
            entry(2, "main", HistoryKind::Assistant, reply),
            tool_call,
            tool_result,
        ],
        rewind_groups: Vec::new(),
        next_before: None,
    };
    app.update(Update::Loaded(
        app.selection(),
        snapshot(&app, "idle", json!(null), json!(null)),
        page,
    ));
    let view = screen(&mut app, 72, 25);
    assert!(view.contains("example_custom · ok · exit 0"));
    assert!(view.contains("before␛[2Jafter"));
    assert!(view.contains("let number = 4;"));
    assert!(!view.contains('\u{001b}'));
    key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
    let view = screen(&mut app, 72, 25);
    assert!(view.contains("Ctrl+O show"));
    assert!(!view.contains("before␛[2Jafter"));
    key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert!(screen(&mut app, 72, 25).contains("before␛[2Jafter"));
    message_action(&mut app, 2, MessageAction::Copy);
    assert!(
        app.effects
            .iter()
            .any(|effect| matches!(effect, Job::Copy(text) if text == reply))
    );
}

#[test]
fn user_card_keeps_its_surface_to_the_right_margin() {
    let mut app = app();
    app.pane_mut()
        .transcript
        .apply(entry(1, "main", HistoryKind::User, "甲\n乙"), None);
    let mut terminal = Terminal::new(TestBackend::new(40, 14)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    for token in ["甲", "乙"] {
        let (_, row) = cell(buffer, token);
        assert_eq!(buffer[(38, row)].bg, crate::theme::SURFACE);
        assert_eq!(buffer[(38, row)].symbol(), "│");
    }
}

#[test]
fn mouse_drag_highlights_and_copies_visible_unicode_in_both_directions() {
    let mut app = app();
    app.pane_mut().transcript.apply(
        entry(
            1,
            "main",
            HistoryKind::Assistant,
            "before **中e\u{301}**\n\n👩‍💻! after",
        ),
        None,
    );
    let mut terminal = Terminal::new(TestBackend::new(72, 22)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let wide = cell(buffer, "中");
    let start = (wide.0 + 1, wide.1);
    let end = cell(buffer, "!");
    let emoji = cell(buffer, "👩‍💻");
    let left = MouseButton::Left;
    let modifiers = KeyModifiers::NONE;

    for (anchor, release) in [(start, end), (end, start)] {
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        app.event(mouse(MouseEventKind::Down(left), anchor, modifiers), buffer);
        app.event(mouse(MouseEventKind::Drag(left), emoji, modifiers), buffer);
        let buffer = terminal
            .draw(|frame| render(frame, &mut app))
            .unwrap()
            .buffer;
        assert!(buffer[emoji].modifier.contains(Modifier::REVERSED));
        if anchor == start {
            assert!(buffer[wide].modifier.contains(Modifier::REVERSED));
            assert!(buffer[start].modifier.contains(Modifier::REVERSED));
            assert!(!buffer[end].modifier.contains(Modifier::REVERSED));
        }
        assert!(app.effects.is_empty());
        app.event(mouse(MouseEventKind::Up(left), release, modifiers), buffer);
        assert!(matches!(
            std::mem::take(&mut app.effects).as_slice(),
            [Job::Copy(text)] if text == "中e\u{301}\n\n👩‍💻!"
        ));
        app.event(mouse(MouseEventKind::Up(left), release, modifiers), buffer);
        assert!(app.effects.is_empty());
    }
}

#[test]
fn mouse_drag_copies_sanitized_text_from_tool_output_popup() {
    let mut app = app();
    app.modal = Some(Modal::Output(Box::new(OutputViewer::new(
        1,
        ToolView {
            name: "Tool".into(),
            summary: "Finished".into(),
            invocation: String::new(),
            details: "before\u{001b}[2J\nExit".into(),
            diff: String::new(),
            output_id: None,
            evidence: String::new(),
            inline_output_complete: false,
            ..ToolView::default()
        },
    ))));
    let mut terminal = Terminal::new(TestBackend::new(58, 18)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let start = cell(buffer, "␛");
    let end = cell(buffer, "E");
    let left = MouseButton::Left;
    let modifiers = KeyModifiers::NONE;
    app.event(mouse(MouseEventKind::Down(left), start, modifiers), buffer);
    app.event(mouse(MouseEventKind::Drag(left), end, modifiers), buffer);
    app.event(mouse(MouseEventKind::Up(left), end, modifiers), buffer);
    assert!(matches!(app.effects.as_slice(), [Job::Copy(text)] if text == "␛[2J\nE"));
}

#[test]
fn picker_drag_copies_and_click_switches_to_the_scrolled_model_row() {
    let mut app = app();
    let mut choices: Vec<_> = (0..14)
        .map(|index| {
            let tier = format!("route-{index}");
            Choice::new(&tier, "Configured tier", ChoiceKind::Close)
        })
        .collect();
    choices.push(Choice::new(
        "Model route",
        "XYZ",
        ChoiceKind::ModelRoute {
            current: false,
            tier: Box::new(kuku_client::TierSummary {
                name: "proxy".into(),
                connection: "gateway".into(),
                model: "fixture".into(),
                purpose: None,
                settings: None,
                output: None,
                request_url: None,
                capabilities: None,
                error: None,
            }),
        },
    ));
    app.picker(PickerKind::Models, choices).move_by(14);
    let mut terminal = Terminal::new(TestBackend::new(82, 24)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let start = cell(buffer, "X");
    let end = cell(buffer, "Z");
    let left = MouseButton::Left;
    let modifiers = KeyModifiers::NONE;
    app.event(mouse(MouseEventKind::Down(left), start, modifiers), buffer);
    app.event(mouse(MouseEventKind::Drag(left), end, modifiers), buffer);
    assert!(app.effects.is_empty());
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    assert!(buffer[start].modifier.contains(Modifier::REVERSED));
    app.event(mouse(MouseEventKind::Up(left), end, modifiers), buffer);
    assert!(matches!(std::mem::take(&mut app.effects).as_slice(),
        [Job::Copy(text)] if text == "XYZ"));
    assert!(matches!(app.modal, Some(Modal::Picker(_))));

    click(&mut app, buffer, start);
    assert!(app.modal.is_none());
    assert_eq!(Some("proxy"), app.pane().config().tier.as_deref());
    assert!(matches!(
        app.effects.as_slice(),
        [Job::ClosePicker, Job::Model(..)]
    ));
}

#[test]
fn picker_click_cannot_switch_agents_when_the_visible_choice_was_replaced() {
    let mut app = app();
    app.pane_mut().exists = true;
    let workspace = app.workspace.clone();
    let session_id = app.active.session.clone();
    let choice = |agent: &str| {
        Choice::new(
            "评审者",
            "Ready",
            ChoiceKind::Agent {
                target: Target {
                    workspace: workspace.clone(),
                    session_id: session_id.clone(),
                    agent: agent.into(),
                },
            },
        )
    };
    app.picker(PickerKind::Agents, vec![choice("reviewer")]);
    let mut terminal = Terminal::new(TestBackend::new(39, 11)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let point = cell(buffer, "评");
    let left = MouseButton::Left;
    let modifiers = KeyModifiers::NONE;
    app.event(mouse(MouseEventKind::Down(left), point, modifiers), buffer);
    let Some(Modal::Picker(picker)) = &mut app.modal else {
        panic!("expected the Agent picker")
    };
    picker.set_local(vec![choice("tester")]);
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    assert_eq!(point, cell(buffer, "评"));
    app.event(mouse(MouseEventKind::Up(left), point, modifiers), buffer);
    assert_eq!("main", app.active.agent);
    assert!(app.effects.is_empty());

    click(&mut app, buffer, point);
    assert_eq!("tester", app.active.agent);
    assert!(app.modal.is_none());
    assert!(app.effects.iter().all(|job| !matches!(job, Job::Save(_))));
}

#[path = "tests/inline.rs"]
mod inline;

#[test]
fn click_shift_drag_and_replaced_text_leave_the_clipboard_alone() {
    let mut app = app();
    app.pane_mut().draft.insert("abc XYZ");
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let left = MouseButton::Left;
    let start = cell(buffer, "X");
    let end = cell(buffer, "Z");
    let down = mouse(MouseEventKind::Down(left), start, KeyModifiers::NONE);
    let up = mouse(MouseEventKind::Up(left), end, KeyModifiers::NONE);
    app.event(down.clone(), buffer);
    app.event(
        mouse(MouseEventKind::Up(left), start, KeyModifiers::NONE),
        buffer,
    );
    for (kind, point) in [
        (MouseEventKind::Down(left), start),
        (MouseEventKind::Drag(left), end),
        (MouseEventKind::Up(left), end),
    ] {
        app.event(mouse(kind, point, KeyModifiers::SHIFT), buffer);
    }
    assert!(app.effects.is_empty());

    app.event(down, buffer);
    app.event(
        mouse(MouseEventKind::Drag(left), end, KeyModifiers::NONE),
        buffer,
    );
    app.pane_mut().draft.set_text("abc UVW".into());
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    app.event(up, buffer);
    assert!(app.effects.is_empty());
    assert!(!buffer[start].modifier.contains(Modifier::REVERSED));
}
