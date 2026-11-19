use std::path::PathBuf;
mod activity;
mod contextual;
mod reading;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use kuku_client::{
    Acknowledgement, Action, Command, Error, HistoryEntry, HistoryKind, HistoryPage, Input,
    InputSource, InputStatus, Observation, QueuedInput, Receipt, Snapshot, Target, Timing,
};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use super::{render, render_at, ActivityFrame};
use crate::app::{Address, App, Modal};
use crate::jobs::{Job, Update};
use crate::menu::{Choice, ChoiceKind, PickerKind, PromptKind};
use crate::output::OutputViewer;
use crate::transcript::ToolView;

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
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<Vec<_>>()
        .chunks(usize::from(width))
        .map(|row| row.concat())
        .collect::<Vec<_>>()
        .join("\n");
    (text, visible)
}

fn app() -> App {
    let workspace = PathBuf::from("/workspace");
    let mut app = App::new(workspace.clone(), None);
    app.update(Update::Connected {
        endpoint: "http://127.0.0.1:6789".into(),
        workspace,
    });
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

#[test]
fn composer_click_places_the_caret_in_scrolled_and_wrapped_unicode_text() {
    let mut app = app();
    let mut terminal = Terminal::new(TestBackend::new(30, 20)).unwrap();
    let prefix = "earlier\n".repeat(6) + &"x".repeat(24);
    app.pane_mut()
        .draft
        .insert(&format!("{prefix}中文e\u{301}👨‍👩‍👧‍👦end"));
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let (x, y) = cell(buffer, "中");
    assert!(y > app.composer_area.y);
    app.event(
        Event::Resize(buffer.area.width, buffer.area.height + 8),
        buffer,
    );
    app.event(Event::Resize(buffer.area.width, buffer.area.height), buffer);
    click(&mut app, buffer, (x + 1, y));
    key(&mut app, KeyCode::Char('!'), KeyModifiers::NONE);
    assert_eq!(
        app.pane().draft.text(),
        format!("{prefix}!中文e\u{301}👨‍👩‍👧‍👦end")
    );
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let point = cell(buffer, "👨‍👩‍👧‍👦");
    click(&mut app, buffer, point);
    key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(
        app.pane().draft.text(),
        format!("{prefix}!中文e\u{301}?👨‍👩‍👧‍👦end")
    );
}

fn command(app: &mut App, name: &str) {
    let draft = app.pane_mut().draft.take_text();
    key(app, KeyCode::Char('/'), KeyModifiers::NONE);
    for character in name.chars() {
        key(app, KeyCode::Char(character), KeyModifiers::NONE);
    }
    key(app, KeyCode::Enter, KeyModifiers::NONE);
    app.pane_mut().draft.set_text(draft);
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
    assert!(screen(&mut app, 92, 25).contains("Queued · steer"));

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
        accepted.contains("Turn 1")
            && accepted
                .lines()
                .any(|line| line.contains("│") && line.contains("You"))
    );
    assert!(!accepted.contains("Pending inputs"));
    assert_eq!(2, accepted.matches(text).count());
}

#[test]
fn blank_welcome_scales_down_and_active_session_uses_one_branded_row() {
    let mut app = app();
    for width in [64, 118] {
        let welcome = screen(&mut app, width, 28);
        let rows: Vec<_> = welcome.lines().collect();
        assert_eq!("", rows[0].trim());
        assert_eq!("⢎⡱  kuku", rows[1].trim());
        let indent = " ".repeat(rows[1].split_once("kuku").unwrap().0.width());
        assert_eq!(format!("{indent}New session"), rows[2].trim_end());
        assert!(rows[4].starts_with(&format!("{indent}Enter sends your first prompt")));
        if width == 64 {
            assert!(rows[5].starts_with(&format!("{indent}/resume opens a saved session")));
        } else {
            assert!(rows[4].contains("/resume opens a saved session"));
            assert!(rows[5].contains("Workspace: /workspace"));
            assert!(rows[6].contains("/model is session-only; config.toml sets defaults."));
        }
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
        .find(|line| line.contains("Workspace: "))
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
        next_before: Some(1),
    };
    app.update(Update::Loaded(
        main_selection.clone(),
        snapshot(&app, "running", json!(null), json!(null)),
        page,
    ));
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
    assert!(other.contains("To reviewer"));
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

    let main_draft = app.pane_mut().draft.take_text();
    key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
    for character in "sou".chars() {
        key(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
    }
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker)) if picker.query.text() == "/source"));
    app.effects.clear();
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    app.pane_mut().draft.set_text(main_draft);
    assert!(matches!(app.effects.as_slice(), [Job::ClosePicker]));
    let source = screen(&mut app, 100, 24);
    assert!(source.contains("Assistant · source"));
    assert!(source.contains("````markdown") && source.contains("```rust"));
    assert!(source.contains("main draft"));

    command(&mut app, "copy");
    assert!(app
        .effects
        .iter()
        .any(|job| matches!(job, Job::Copy(text) if text == raw)));
    app.select(reviewer, true, "Example".into());
    let other = screen(&mut app, 100, 24);
    assert!(other.contains("Reviewer reply") && other.contains("reviewer draft"));
    assert!(!other.contains("**Reviewer reply**"));
    app.select(main, true, "Example".into());
    assert!(screen(&mut app, 100, 24).contains("````markdown"));
    command(&mut app, "source");
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
    command(&mut app, "copy");
    assert!(app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::Copy(text) if text == reply)));
}

#[test]
fn short_and_wrapped_user_lines_share_a_flat_surface_to_the_right_margin() {
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
        assert_eq!(buffer[(38, row)].symbol(), " ");
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
fn picker_drag_copies_and_click_opens_the_scrolled_model_detail_row() {
    let mut app = app();
    let mut choices: Vec<_> = (0..14)
        .map(|index| {
            let tier = format!("route-{index}");
            Choice::new(&tier, "Configured tier", ChoiceKind::Tier(tier.clone()))
        })
        .collect();
    choices.push(Choice::new(
        "Connection",
        "XYZ",
        ChoiceKind::Connection("proxy".into()),
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
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::Model { connection } if connection == "proxy")));
    assert!(matches!(app.effects.as_slice(), [Job::ClosePicker]));
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

#[test]
fn spacious_composer_and_picker_use_responsive_insets() {
    let mut app = app();
    app.pane_mut().draft.insert("甲");
    let mut wide = Terminal::new(TestBackend::new(128, 32)).unwrap();
    let mut compact = Terminal::new(TestBackend::new(44, 12)).unwrap();
    let wide_frame = wide.draw(|frame| render(frame, &mut app)).unwrap();
    let (wide_x, wide_y) = cell(wide_frame.buffer, "甲");
    assert!(wide_x >= 6);
    assert_eq!(
        wide_frame.buffer[(wide_x, wide_y + 1)].bg,
        crate::theme::SURFACE
    );
    let compact_frame = compact.draw(|frame| render(frame, &mut app)).unwrap();
    let (compact_x, compact_y) = cell(compact_frame.buffer, "甲");
    assert!(compact_x < wide_x);
    assert_eq!(compact_y, 9);

    app.picker(PickerKind::Commands, Choice::commands());
    let frame = wide.draw(|frame| render(frame, &mut app)).unwrap();
    let (x, y) = cell(frame.buffer, "›");
    assert_ne!(frame.buffer[(x, y)].bg, frame.buffer[(x - 1, y)].bg);
    assert!(!frame.buffer[(x, y)].modifier.contains(Modifier::DIM));
    assert!(frame.buffer[(0, 0)].modifier.contains(Modifier::DIM));
}
