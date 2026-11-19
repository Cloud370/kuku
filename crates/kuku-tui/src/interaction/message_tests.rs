use crossterm::event::KeyCode;
use kuku_client::HistoryKind;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

use super::tests::{
    choose_message_action, click_screen, connected_app, draw, insert_message, open_message_actions,
    press, screen_position, transcript_text,
};
use crate::app::Modal;
use crate::jobs::{Job, Update};
use crate::menu::{ChoiceKind, MessageAction, PickerKind};
use crate::transcript::{MessageId, SelectedMessage};

#[test]
fn messages_select_older_assistant_and_source_only_that_entry() {
    let mut app = connected_app();
    insert_message(
        &mut app,
        41,
        HistoryKind::Assistant,
        "**older reply**",
        Default::default(),
    );
    insert_message(
        &mut app,
        42,
        HistoryKind::Assistant,
        "**newer reply**",
        Default::default(),
    );
    app.paste("preserved draft");

    app.open_messages_picker();
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Messages)
            && picker.visible_choices().len() == 2));
    press(&mut app, KeyCode::Down);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.selected_choice(), Some(ChoiceKind::MessageEntry(SelectedMessage { id: MessageId::Event(41), .. })))));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::MessageActions)
            && picker.visible_choices().iter().any(|choice| choice.label == "Source")
            && picker.header.contains("older reply")));
    press(&mut app, KeyCode::Tab);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.selected_choice(), Some(ChoiceKind::MessageAction { action: MessageAction::Source, .. }))));
    press(&mut app, KeyCode::Enter);

    let rendered = transcript_text(&mut app);
    assert!(rendered.contains("**older reply**"), "{rendered}");
    assert!(!rendered.contains("**newer reply**"), "{rendered}");
    assert_eq!("preserved draft", app.pane().draft.text());

    open_message_actions(&mut app, MessageId::Event(41));
    choose_message_action(&mut app, MessageAction::Source);
    let preview = transcript_text(&mut app);
    assert!(!preview.contains("**older reply**"), "{preview}");
    assert!(preview.contains("older reply"), "{preview}");
}

#[test]
fn copy_uses_selected_entry_and_fetches_its_exact_untruncated_source() {
    let mut app = connected_app();
    let original = format!("older selected reply\n{}", "x".repeat(180_000));
    insert_message(
        &mut app,
        51,
        HistoryKind::Assistant,
        &original,
        Default::default(),
    );
    insert_message(
        &mut app,
        52,
        HistoryKind::Assistant,
        "newest reply",
        Default::default(),
    );
    let selection = app.selection();
    open_message_actions(&mut app, MessageId::Event(51));
    choose_message_action(&mut app, MessageAction::Copy);
    assert!(matches!(
        app.effects.as_slice(),
        [Job::ClosePicker, Job::ClosePicker, Job::ReplySource(target, 51)]
            if target == &selection
    ));

    app.effects.clear();
    app.update(Update::ReplySource(selection, 51, Ok(original.clone())));
    assert!(matches!(app.effects.as_slice(), [Job::Copy(text)] if text == &original));

    app.effects.clear();
    open_message_actions(&mut app, MessageId::Event(52));
    choose_message_action(&mut app, MessageAction::Copy);
    assert!(matches!(
        app.effects.as_slice(),
        [Job::ClosePicker, Job::ClosePicker, Job::Copy(text)] if text == "newest reply"
    ));
}

#[test]
fn thinking_is_available_only_for_its_associated_message() {
    let mut app = connected_app();
    let details =
        serde_json::from_value(json!({"reasoning_preview":"older private thought"})).unwrap();
    insert_message(
        &mut app,
        61,
        HistoryKind::Assistant,
        "older response",
        details,
    );
    insert_message(
        &mut app,
        62,
        HistoryKind::Assistant,
        "newer response",
        Default::default(),
    );

    open_message_actions(&mut app, MessageId::Event(61));
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if picker.visible_choices().iter().any(|choice| choice.label == "Thinking")));
    choose_message_action(&mut app, MessageAction::Thinking);
    assert!(transcript_text(&mut app).contains("older private thought"));

    open_message_actions(&mut app, MessageId::Event(62));
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if !picker.visible_choices().iter().any(|choice| choice.label == "Thinking")));
}

#[test]
fn tool_actions_expand_the_selected_call_and_open_saved_or_complete_inline_output() {
    let mut app = connected_app();
    for event in [
        json!({"id":71,"agent":"main","turn":3,"type":"tool_call","tool":"run_command","tool_call_id":"exact-call","args":{"command":"echo selected","brief":"selected command"}}),
        json!({"id":72,"agent":"main","turn":3,"type":"tool_result","tool":"run_command","tool_call_id":"exact-call","status":"ok","summary":"selected command complete","model_content":"short preview","output_id":"saved-exact-output"}),
        json!({"id":73,"agent":"main","turn":4,"type":"tool_call","tool":"read_file","tool_call_id":"pending-call","args":{"path":"pending.txt"}}),
        json!({"id":74,"agent":"main","turn":5,"type":"tool_call","tool":"read_file","tool_call_id":"inline-result","args":{"path":"inline.txt"}}),
        json!({"id":75,"agent":"main","turn":5,"type":"tool_result","tool":"read_file","tool_call_id":"inline-result","status":"ok","summary":"Read inline.txt","model_content":"complete inline file contents"}),
        json!({"id":76,"agent":"main","turn":6,"type":"tool_call","tool":"read_file","tool_call_id":"truncated-result","args":{"path":"partial.txt"}}),
        json!({"id":77,"agent":"main","turn":6,"type":"tool_result","tool":"read_file","tool_call_id":"truncated-result","status":"ok","summary":"Read partial.txt","model_content":"partial inline contents","truncated":true}),
    ] {
        app.pane_mut()
            .transcript
            .apply(serde_json::from_value(event).unwrap(), None);
    }
    let id = MessageId::Tool {
        turn: Some(3),
        call_id: "exact-call".into(),
    };
    open_message_actions(&mut app, id.clone());
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if picker.visible_choices().iter().any(|choice| choice.label == "Expand")
            && picker.visible_choices().iter().any(|choice| choice.label == "Full output")
            && !picker.visible_choices().iter().any(|choice| choice.label == "Source")));
    choose_message_action(&mut app, MessageAction::Expand);
    assert!(transcript_text(&mut app).contains("echo selected"));

    open_message_actions(&mut app, id);
    choose_message_action(&mut app, MessageAction::FullOutput);
    assert!(matches!(
        &app.modal,
        Some(Modal::Output(viewer)) if viewer.tool.output_id.as_deref() == Some("saved-exact-output")
    ));
    assert!(app.effects.iter().any(|effect| matches!(
        effect,
        Job::Output(selection, _, id, 0)
            if selection == &app.selection() && id == "saved-exact-output"
    )));

    app.modal = None;
    open_message_actions(
        &mut app,
        MessageId::Tool {
            turn: Some(4),
            call_id: "pending-call".into(),
        },
    );
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if !picker.visible_choices().iter().any(|choice| choice.label == "Full output")));

    app.modal = None;
    open_message_actions(
        &mut app,
        MessageId::Tool {
            turn: Some(5),
            call_id: "inline-result".into(),
        },
    );
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if picker.visible_choices().iter().any(|choice| choice.label == "Full output")));
    choose_message_action(&mut app, MessageAction::FullOutput);
    let Some(Modal::Output(viewer)) = &mut app.modal else {
        panic!("inline output viewer expected")
    };
    assert!(!viewer.loading);
    assert_eq!(None, viewer.tool.output_id);
    let rendered = viewer
        .render_lines(80, 20)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("complete inline file contents"),
        "{rendered}"
    );

    app.modal = None;
    open_message_actions(
        &mut app,
        MessageId::Tool {
            turn: Some(6),
            call_id: "truncated-result".into(),
        },
    );
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if !picker.visible_choices().iter().any(|choice| choice.label == "Full output")));
}

#[test]
fn remediation_expand_opens_a_nonfirst_tool_in_a_collapsed_group() {
    let mut app = connected_app();
    for event in [
        json!({"id":81,"agent":"main","turn":8,"type":"tool_call","tool":"read_file","tool_call_id":"first-read","args":{"path":"first.txt"}}),
        json!({"id":82,"agent":"main","turn":8,"type":"tool_result","tool":"read_file","tool_call_id":"first-read","status":"ok","summary":"Read first.txt","model_content":"FIRST-CONTENT"}),
        json!({"id":83,"agent":"main","turn":8,"type":"tool_call","tool":"read_file","tool_call_id":"second-read","args":{"path":"second.txt"}}),
        json!({"id":84,"agent":"main","turn":8,"type":"tool_result","tool":"read_file","tool_call_id":"second-read","status":"ok","summary":"Read second.txt","model_content":"SECOND-UNIQUE-CONTENT"}),
    ] {
        app.pane_mut()
            .transcript
            .apply(serde_json::from_value(event).unwrap(), None);
    }

    open_message_actions(
        &mut app,
        MessageId::Tool {
            turn: Some(8),
            call_id: "second-read".into(),
        },
    );
    choose_message_action(&mut app, MessageAction::Expand);

    let rendered = transcript_text(&mut app);
    assert!(rendered.contains("SECOND-UNIQUE-CONTENT"), "{rendered}");
}

#[test]
fn escape_from_expand_action_returns_to_the_selected_message() {
    let mut app = connected_app();
    for event in [
        json!({"id":71,"agent":"main","turn":3,"type":"tool_call","tool":"run_command","tool_call_id":"exact-call","args":{"command":"echo selected","brief":"selected command"}}),
        json!({"id":72,"agent":"main","turn":3,"type":"tool_result","tool":"run_command","tool_call_id":"exact-call","status":"ok","summary":"selected command complete","model_content":"short preview","output_id":"saved-exact-output"}),
    ] {
        app.pane_mut()
            .transcript
            .apply(serde_json::from_value(event).unwrap(), None);
    }
    let id = MessageId::Tool {
        turn: Some(3),
        call_id: "exact-call".into(),
    };
    open_message_actions(&mut app, id.clone());
    let Some(Modal::Picker(picker)) = &mut app.modal else {
        panic!("message actions picker expected")
    };
    picker.selected = picker
        .visible_choices()
        .iter()
        .position(|choice| {
            matches!(&choice.kind, ChoiceKind::Detail { message: Some(message), .. } if message.id == id)
        })
        .expect("selected message expand action is available");

    press(&mut app, KeyCode::Esc);

    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Messages)
            && matches!(picker.selected_choice(), Some(ChoiceKind::MessageEntry(message)) if message.id == id)));
}

#[test]
fn message_picker_escape_preserves_draft_and_stale_actions_fail_safe() {
    let mut empty = connected_app();
    empty.paste("keep this draft");
    empty.open_messages_picker();
    assert!(matches!(&empty.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Messages) && picker.visible_choices().is_empty()));
    press(&mut empty, KeyCode::Esc);
    assert!(empty.modal.is_none());
    assert_eq!("keep this draft", empty.pane().draft.text());

    let mut app = connected_app();
    insert_message(
        &mut app,
        81,
        HistoryKind::Assistant,
        "selected response",
        Default::default(),
    );
    app.paste("draft survives both levels");
    open_message_actions(&mut app, MessageId::Event(81));
    press(&mut app, KeyCode::Esc);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Messages)));
    assert_eq!("draft survives both levels", app.pane().draft.text());
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
    assert_eq!("draft survives both levels", app.pane().draft.text());

    open_message_actions(&mut app, MessageId::Event(81));
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("message action picker expected")
    };
    let Some(ChoiceKind::MessageAction { mut message, .. }) = picker.selected_choice() else {
        panic!("copy action expected")
    };
    message.selection.generation = message.selection.generation.wrapping_add(1);
    app.execute_message_action(message, MessageAction::Copy);
    assert!(!app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::Copy(_))));
    assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
    assert_eq!("draft survives both levels", app.pane().draft.text());

    let mut target = app.selection().target;
    target.agent = "reviewer".into();
    app.select_agent(target);
    choose_message_action(&mut app, MessageAction::Copy);
    assert!(!app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::Copy(_))));
}

#[test]
fn message_actions_fail_closed_after_rewind_removes_the_entry() {
    let mut app = connected_app();
    let original = format!("rewound response\n{}", "x".repeat(180_000));
    insert_message(
        &mut app,
        91,
        HistoryKind::Assistant,
        &original,
        Default::default(),
    );
    let selection = app.selection();
    open_message_actions(&mut app, MessageId::Event(91));
    choose_message_action(&mut app, MessageAction::Copy);
    assert!(app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::ReplySource(target, 91) if target == &selection)));
    app.effects.clear();
    app.pane_mut().transcript.invalidate_after_rewind();
    app.update(Update::ReplySource(selection, 91, Ok(original)));
    assert!(!app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::Copy(_))));
    assert_eq!(None, app.pane().pending_reply_copy);
}

#[test]
fn mouse_opens_messages_selects_entry_and_runs_action_without_losing_draft() {
    let mut app = connected_app();
    insert_message(
        &mut app,
        101,
        HistoryKind::User,
        "mouse-target user entry",
        Default::default(),
    );
    app.paste("draft remains editable");
    app.effects.clear();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let screen = draw(&mut app, &mut terminal);
    click_screen(&mut app, &screen, screen_position(&screen, "Messages"));
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Messages)));

    let screen = draw(&mut app, &mut terminal);
    let _ = screen_position(&screen, "Operate message");
    click_screen(
        &mut app,
        &screen,
        screen_position(&screen, "mouse-target user entry"),
    );
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::MessageActions)
            && picker.header.contains("mouse-target user entry")));

    let screen = draw(&mut app, &mut terminal);
    click_screen(&mut app, &screen, screen_position(&screen, "Copy"));
    assert!(app
        .effects
        .iter()
        .any(|effect| matches!(effect, Job::Copy(text) if text == "mouse-target user entry")));
    assert_eq!("draft remains editable", app.pane().draft.text());

    for width in [32, 33] {
        let mut compact = Terminal::new(TestBackend::new(width, 30)).unwrap();
        let screen = draw(&mut app, &mut compact);
        let message_button = screen_position(&screen, "Msg");
        click_screen(&mut app, &screen, message_button);
        assert!(matches!(&app.modal, Some(Modal::Picker(picker))
            if matches!(picker.kind, PickerKind::Messages)));
        app.modal = None;
    }
}

#[test]
fn message_and_rewind_picker_footers_name_their_distinct_modes() {
    let mut app = connected_app();
    app.open_messages_picker();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let screen = draw(&mut app, &mut terminal);
    let _ = screen_position(&screen, "Operate message");

    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if matches!(picker.kind, PickerKind::Rewind)));
    let screen = draw(&mut app, &mut terminal);
    let _ = screen_position(&screen, "Rewind ·");
}

#[test]
fn message_picker_excludes_entries_outside_the_active_window() {
    let mut app = connected_app();
    let original = "x".repeat(80_000);
    insert_message(
        &mut app,
        111,
        HistoryKind::Assistant,
        &original,
        Default::default(),
    );
    for id in 112..1200 {
        insert_message(
            &mut app,
            id,
            HistoryKind::User,
            "later input",
            Default::default(),
        );
    }
    assert!(!app
        .pane()
        .transcript
        .has_assistant_message(111, &app.active.agent));

    app.open_messages_picker();
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("message picker expected");
    };
    assert!(!picker.visible_choices().iter().any(|choice| matches!(
        &choice.kind,
        ChoiceKind::MessageEntry(SelectedMessage {
            id: MessageId::Event(111),
            ..
        })
    )));
}
