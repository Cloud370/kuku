use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use kuku_client::{
    Action, Error, HistoryEntry, HistoryKind, HistoryPage, Observation, Snapshot, Timing,
};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::{Frame, Terminal};
use serde_json::json;

use super::render as render_with_statusline;
use crate::app::{Address, App, Modal};
use crate::jobs::{Job, Update};
use crate::output::OutputViewer;
use crate::statusline::StatusLine;
use crate::transcript::ToolView;

fn render(frame: &mut Frame<'_>, app: &mut App) {
    render_with_statusline(frame, app, &StatusLine::default());
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
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

fn cell(buffer: &Buffer, symbol: &str) -> (u16, u16) {
    let index = buffer
        .content
        .iter()
        .position(|cell| cell.symbol() == symbol)
        .unwrap_or_else(|| panic!("missing screen symbol {symbol:?}"));
    buffer.pos_of(index)
}

fn command(app: &mut App, name: &str) {
    key(app, KeyCode::F(1), KeyModifiers::NONE);
    for character in name.chars() {
        key(app, KeyCode::Char(character), KeyModifiers::NONE);
    }
    key(app, KeyCode::Enter, KeyModifiers::NONE);
}

fn entry(id: u64, agent: &str, kind: HistoryKind, text: &str) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: agent.into(),
        turn: Some(1),
        kind,
        text: Some(text.into()),
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
    assert!(view.contains("To main"));
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
        before.lines().take(12).collect::<Vec<_>>(),
        after.lines().take(12).collect::<Vec<_>>()
    );
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
    assert!(!app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::Save(_))));
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
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
        next_before: None,
    };
    app.update(Update::Loaded(
        app.selection(),
        snapshot(&app, "idle", json!(null), json!(null)),
        page,
    ));
    let view = screen(&mut app, 72, 25);
    assert!(view.contains("Tool example_custom · ok · exit 0"));
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
