use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kuku_client::{Acknowledgement, Action, Receipt};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

use crate::app::{App, Modal};
use crate::jobs::{Job, Search, Update};
use crate::menu::{Choice, ChoiceKind, PickerKind, PromptKind};

fn connected_app() -> App {
    let workspace = PathBuf::from("/workspace");
    let mut app = App::new(workspace.clone(), None);
    app.update(Update::Connected {
        endpoint: "http://127.0.0.1:6789".into(),
        workspace,
    });
    app.effects.clear();
    app
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn help_shortcuts_are_read_only_and_slash_opens_actions() {
    let mut app = connected_app();
    app.paste("unsent draft");
    for key in [
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
    ] {
        app.key(key);
        assert!(matches!(&app.modal, Some(Modal::Help)));
        assert_eq!("unsent draft", app.pane().draft.text());
        app.paste("ignored paste");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(&app.modal, Some(Modal::Help)));
        assert_eq!("unsent draft", app.pane().draft.text());
        assert!(app.effects.is_empty());
        press(&mut app, KeyCode::Esc);
        assert!(app.effects.is_empty());
    }
    press(&mut app, KeyCode::Char('/'));
    assert_eq!("unsent draft/", app.pane().draft.text());
    assert!(app.modal.is_none());
    app.pane_mut().draft.take_text();
    press(&mut app, KeyCode::Char('/'));
    assert!(matches!(&app.modal,
        Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Commands)
    ));
    assert!(app.pane().draft.text().is_empty());
}

#[test]
fn skill_named_like_an_action_remains_a_distinct_editable_choice() {
    let mut app = connected_app();
    let mut choices = Choice::commands();
    choices.push(Choice::new(
        "/skill:new",
        "Skill · new",
        ChoiceKind::Skill("new".into()),
    ));
    assert!(choices
        .iter()
        .any(|choice| { choice.label == "/new" && matches!(choice.kind, ChoiceKind::Command(_)) }));
    assert!(choices.iter().any(|choice| {
        choice.label == "/skill:new" && matches!(choice.kind, ChoiceKind::Skill(_))
    }));
    app.picker(PickerKind::Commands, choices);
    app.paste("/skill:new");
    press(&mut app, KeyCode::Tab);
    assert_eq!("/skill:new ", app.pane().draft.text());
    assert!(app.modal.is_none());
}

#[test]
fn plain_arrows_preserve_the_draft_and_history_requires_an_explicit_shortcut() {
    let mut app = connected_app();
    app.pane_mut().draft.remember("Earlier prompt");
    app.paste("first\nsecond");
    press(&mut app, KeyCode::Up);
    assert_eq!("first\nsecond", app.pane().draft.text());
    assert_eq!(5, app.pane().draft.cursor());
    press(&mut app, KeyCode::Up);
    assert_eq!("first\nsecond", app.pane().draft.text());
    app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!("Earlier prompt", app.pane().draft.text());
    app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!("Earlier prompt", app.pane().draft.text());
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!("first\nsecond", app.pane().draft.text());
    assert_eq!(5, app.pane().draft.cursor());
    press(&mut app, KeyCode::Down);
    assert_eq!("first\nsecond", app.pane().draft.text());
    assert_eq!(11, app.pane().draft.cursor());
}

fn agent_cards(app: &mut App) {
    for (index, mut event) in [
        json!({
            "type":"tool_call", "tool":"create_agent", "tool_call_id":"create",
            "args":{"address":"reviewer","instructions":"Inspect documentation\nUse precise citations"}
        }),
        json!({
            "type":"tool_result", "tool_call_id":"create", "status":"ok",
            "summary":"Session operation committed",
            "structured":{"agent":"reviewer","command_id":"tool:main:7:create","event_id":1}
        }),
        json!({
            "type":"tool_call", "tool":"send_message", "tool_call_id":"send",
            "args":{"to":"reviewer","message":"Review the CLI\nList failing cases"}
        }),
        json!({
            "type":"tool_result", "tool_call_id":"send", "status":"ok",
            "summary":"Session operation committed",
            "structured":{"agent":"reviewer","command_id":"tool:main:7:send","event_id":3}
        }),
    ].into_iter().enumerate() {
        event["id"] = json!(index + 1);
        event["agent"] = json!("main");
        event["turn"] = json!(7);
        event["turn_number"] = json!(1);
        app.pane_mut().transcript.apply(serde_json::from_value(event).unwrap(), None);
    }
}

fn transcript_text(app: &mut App) -> String {
    app.pane_mut()
        .transcript
        .project(96, 32)
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn open_details(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.paste("/details");
    press(app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Details)));
    app.effects.clear();
}

#[test]
fn details_search_selects_one_visible_card_and_escape_leaves_previews_alone() {
    let mut app = connected_app();
    agent_cards(&mut app);
    let collapsed = transcript_text(&mut app);
    assert!(!collapsed.contains("Use precise citations"));
    assert!(!collapsed.contains("List failing cases"));
    open_details(&mut app);
    app.paste("reviewer CLI");
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("details picker expected");
    };
    let choices = picker.visible_choices();
    assert_eq!(1, choices.len());
    assert_eq!("Agent reviewer", choices[0].label);
    assert!(choices[0].detail.contains("Review the CLI"));
    assert!(app.effects.is_empty(), "search only uses visible cards");
    press(&mut app, KeyCode::Esc);
    assert_eq!(collapsed, transcript_text(&mut app));

    open_details(&mut app);
    app.paste("reviewer");
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    let expanded = transcript_text(&mut app);
    assert!(
        expanded.contains("Use precise citations") && expanded.contains("List failing cases"),
        "{expanded}"
    );
    assert!(app.pending.is_empty());
}

#[test]
fn control_o_expands_and_collapses_agent_previews_together() {
    let mut app = connected_app();
    agent_cards(&mut app);
    let collapsed = transcript_text(&mut app);
    let details = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
    app.key(details);
    let expanded = transcript_text(&mut app);
    assert!(expanded.contains("Use precise citations"), "{expanded}");
    assert!(expanded.contains("List failing cases"), "{expanded}");
    app.key(details);
    assert_eq!(collapsed, transcript_text(&mut app));
}

#[test]
fn control_c_clears_dialogs_and_exits_only_from_an_empty_composer() {
    let mut app = connected_app();
    let clear = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.paste("draft to keep");
    app.picker(PickerKind::Commands, Choice::commands());
    app.paste("resume");
    app.effects.clear();
    app.key(clear);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker)) if picker.query.text().is_empty()));
    assert!(
        matches!(app.effects.as_slice(), [Job::Find(_, Search::Skills(query))] if query.is_empty())
    );
    app.key(clear);
    assert!(!app.quit);
    assert_eq!("draft to keep", app.pane().draft.text());
    press(&mut app, KeyCode::Esc);

    app.prompt(PromptKind::Agent, "New agent", "Enter the address");
    app.paste("reviewer");
    app.key(clear);
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt)) if prompt.editor.text().is_empty()));
    app.key(clear);
    assert!(!app.quit);
    assert_eq!("draft to keep", app.pane().draft.text());
    press(&mut app, KeyCode::Esc);

    app.effects.clear();
    app.key(clear);
    assert!(app.pane().draft.text().is_empty());
    assert!(!app.quit);
    app.key(KeyEvent {
        kind: crossterm::event::KeyEventKind::Repeat,
        ..clear
    });
    assert!(!app.quit);
    app.key(clear);
    assert!(app.quit);
    assert!(app.effects.is_empty());
}

#[test]
fn double_escape_opens_rewind_picker_and_confirmation_is_cancelable_and_opt_in() {
    let mut app = connected_app();
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert!(app.escape_armed_at.is_some());

    press(&mut app, KeyCode::Esc);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Rewind)
            && picker.header.contains("Loaded history only")));
    app.choose(
        PickerKind::Rewind,
        ChoiceKind::RewindMessage {
            event_id: 17,
            turn: 4,
            text: "original prompt".into(),
        },
        0,
    );
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(prompt.kind, PromptKind::RewindMessage { event_id: 17, .. })
            && prompt.instructions.contains("[ ] Restore recorded file changes")));

    press(&mut app, KeyCode::Char('x'));
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt)) if prompt.editor.text().is_empty()));
    press(&mut app, KeyCode::Char(' '));
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if prompt.instructions.contains("[x] Restore recorded file changes")));
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert!(app.pending.is_empty());
    assert!(!app.effects.iter().any(|job| matches!(job, Job::Save(_))));
}

#[test]
fn rewind_confirmation_commits_an_unsent_action_without_sending_the_draft() {
    let mut app = connected_app();
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    app.choose(
        PickerKind::Rewind,
        ChoiceKind::RewindMessage {
            event_id: 23,
            turn: 6,
            text: "restore me as a draft".into(),
        },
        0,
    );
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Enter);
    assert!(app.pane().draft.text().is_empty());
    assert!(matches!(app.effects.last(), Some(Job::Save(command))
        if matches!(&command.action, Action::RewindMessage {
            target_event_id: 23,
            text,
            restore_files: true,
        } if text == "restore me as a draft")));
}

#[test]
fn rewind_ack_restores_unsent_text_but_never_replaces_a_changed_draft() {
    let mut app = connected_app();
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    app.choose(
        PickerKind::Rewind,
        ChoiceKind::RewindMessage {
            event_id: 31,
            turn: 8,
            text: "original text".into(),
        },
        0,
    );
    press(&mut app, KeyCode::Enter);
    let Some(Job::Save(command)) = app.effects.iter().find(|job| matches!(job, Job::Save(_)))
    else {
        panic!("rewind command is durably saved")
    };
    let id = command.command_id.clone();
    app.update(Update::Sent(
        id.clone(),
        Ok(Acknowledgement {
            receipt: Receipt {
                command_id: id.clone(),
                event_id: 32,
                agent: "main".into(),
            },
            replayed: false,
            status: None,
            warnings: vec![],
        }),
    ));
    assert_eq!("original text", app.pane().draft.text());
    assert!(!app.effects.iter().any(|job| matches!(job, Job::Send(_))));

    let mut edited = connected_app();
    press(&mut edited, KeyCode::Esc);
    press(&mut edited, KeyCode::Esc);
    edited.choose(
        PickerKind::Rewind,
        ChoiceKind::RewindMessage {
            event_id: 41,
            turn: 9,
            text: "recoverable original".into(),
        },
        0,
    );
    press(&mut edited, KeyCode::Enter);
    let Some(Job::Save(command)) = edited
        .effects
        .iter()
        .find(|job| matches!(job, Job::Save(_)))
    else {
        panic!("rewind command is durably saved")
    };
    let id = command.command_id.clone();
    edited.paste("draft typed while rewind was pending");
    edited.update(Update::Sent(
        id.clone(),
        Ok(Acknowledgement {
            receipt: Receipt {
                command_id: id.clone(),
                event_id: 42,
                agent: "main".into(),
            },
            replayed: false,
            status: None,
            warnings: vec![],
        }),
    ));
    assert_eq!(
        "draft typed while rewind was pending",
        edited.pane().draft.text()
    );
    assert!(!edited.rewind_drafts.contains_key(&id));
    edited.pane_mut().draft.recall(-1);
    assert_eq!("recoverable original", edited.pane().draft.text());
}

#[test]
fn rewind_confirmation_render_shows_file_scope_without_an_editor() {
    let mut app = connected_app();
    app.choose(
        PickerKind::Rewind,
        ChoiceKind::RewindMessage {
            event_id: 99,
            turn: 2,
            text: "selected prompt".into(),
        },
        0,
    );
    let mut terminal = Terminal::new(TestBackend::new(92, 26)).unwrap();
    let frame = terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    let rendered = frame
        .buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<Vec<_>>()
        .join("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let compact = |text: &str| {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
    };
    let warning = "Restores only recorded workspace file changes; shell commands and external side effects are not reversed.";
    assert!(compact(&rendered).contains(&compact(warning)), "{rendered}");
    assert!(rendered.contains("Space Toggle file restore"));
    assert!(!rendered.contains("Type here"));
}

#[test]
fn tab_completes_a_slash_action_and_enter_runs_it_from_an_empty_draft() {
    let mut app = connected_app();
    press(&mut app, KeyCode::Char('/'));
    app.paste("qui");
    press(&mut app, KeyCode::Tab);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker)) if picker.query.text() == "/quit"));
    assert!(app.pane().draft.text().is_empty());
    assert!(!app.quit);
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    assert!(app.quit);
}

#[test]
fn tab_inserts_paths_and_skills_in_the_draft() {
    let mut app = connected_app();
    let path = Choice::new("Cargo.toml", "", ChoiceKind::Path("Cargo.toml".into()));
    app.paste("inspect Car");
    let range = app.pane().draft.completion_range();
    app.picker(PickerKind::Paths(range), vec![path]);
    press(&mut app, KeyCode::Tab);
    assert_eq!("inspect Cargo.toml ", app.pane().draft.text());
    assert!(app.modal.is_none());

    let skill = Choice::new("/skill:review", "", ChoiceKind::Skill("review".into()));
    app.picker(PickerKind::Commands, vec![skill]);
    press(&mut app, KeyCode::Tab);
    assert_eq!("/skill:review inspect Cargo.toml ", app.pane().draft.text());
    assert!(app.modal.is_none());
    assert!(app.pending.is_empty());
}

#[test]
fn copy_fetches_the_complete_committed_reply_and_ignores_older_requests() {
    use kuku_client::{HistoryEntry, HistoryKind, HistoryPage};

    let mut app = connected_app();
    let snapshot = serde_json::from_value(json!({
        "session_id": app.active.session,
        "workspace": app.workspace,
        "cursor": 4,
        "pending": [],
        "agents": [],
        "agent": {
            "address": app.active.agent, "config": {}, "status": "running", "pending_inputs": 0,
            "last_output": "The previous Turn",
            "usage": {"model_requests": 0, "unreported_requests": 0},
        }
    }))
    .unwrap();
    let reply = |id, text: &str| HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(2),
        kind: HistoryKind::Assistant,
        text: Some(text.into()),
        turn_summary: None,
        details: Default::default(),
    };
    let original = format!(
        "```rust\r\n{}```\r\n\r\n结束  \r\n",
        "    let 值 = 42;\r\n".repeat(4000)
    );
    let selection = app.selection();
    app.update(Update::Loaded(
        selection.clone(),
        Box::new(snapshot),
        HistoryPage {
            entries: vec![reply(4, &original)],
            next_before: None,
        },
    ));
    app.effects.clear();
    app.copy_reply();
    assert!(
        matches!(app.effects.as_slice(), [Job::ReplySource(target, 4)] if target == &selection)
    );
    app.effects.clear();

    let mut old_selection = selection.clone();
    old_selection.generation += 1;
    app.update(Update::ReplySource(
        old_selection,
        4,
        Ok("Wrong target".into()),
    ));
    app.update(Update::ReplySource(
        selection.clone(),
        3,
        Ok("Wrong reply".into()),
    ));
    assert!(app.effects.is_empty());
    app.update(Update::ReplySource(
        selection.clone(),
        4,
        Ok(original.clone()),
    ));
    assert!(matches!(app.effects.as_slice(), [Job::Copy(text)] if text == &original));

    app.effects.clear();
    app.copy_reply();
    app.effects.clear();
    let newest = "`Raw` **Markdown**\r\n尾字  \r\n";
    app.pane_mut().transcript.apply(reply(7, newest), None);
    app.copy_reply();
    app.update(Update::ReplySource(selection, 4, Ok(original)));
    assert!(matches!(app.effects.as_slice(), [Job::Copy(text)] if text == newest));
}

#[test]
fn copy_reply_does_not_use_snapshot_fallback_when_history_has_no_reply() {
    use kuku_client::HistoryPage;

    let mut app = connected_app();
    let snapshot = serde_json::from_value(json!({
        "session_id": app.active.session,
        "workspace": app.workspace,
        "cursor": 4,
        "pending": [],
        "agents": [],
        "agent": {
            "address": app.active.agent, "config": {}, "status": "idle", "pending_inputs": 0,
            "last_output": null,
            "usage": {"model_requests": 0, "unreported_requests": 0},
        }
    }))
    .unwrap();
    let selection = app.selection();
    app.update(Update::Loaded(
        selection,
        Box::new(snapshot),
        HistoryPage {
            entries: Vec::new(),
            next_before: None,
        },
    ));
    app.effects.clear();

    app.copy_reply();

    assert!(!app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::Copy(_))));
}
