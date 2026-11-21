use crossterm::event::KeyCode;
use kuku_client::{Action, Error, HistoryEntry, HistoryKind, HistoryPage};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::app::{Address, App, Modal};
use crate::jobs::{Job, Selection, Update};
use crate::menu::{ChoiceKind, Picker, PickerKind, PromptKind, Ticket};
use crate::transcript::{HISTORY_PAGE, MAX_BODY_BYTES, MAX_RECORDS};

use super::tests::{connected_app, press};

fn entry(id: u64) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(id * 10),
        kind: HistoryKind::User,
        text: Some(format!("message {id}")),
        turn_summary: None,
        details: [("turn_number".into(), serde_json::json!(id))]
            .into_iter()
            .collect(),
    }
}

fn page(ids: &[u64], next_before: Option<u64>) -> HistoryPage {
    HistoryPage {
        entries: ids.iter().map(|id| entry(*id)).collect(),
        rewind_groups: Vec::new(),
        next_before,
    }
}

fn app() -> App {
    let mut app = connected_app();
    app.pane_mut()
        .transcript
        .replace_recent(page(&[100, 101, 102], Some(100)));
    app.open_rewind_picker();
    app
}

fn picker(app: &App) -> &Picker {
    match &app.modal {
        Some(Modal::Picker(picker)) => picker,
        _ => panic!("rewind picker expected"),
    }
}

fn selected(app: &App) -> u64 {
    match picker(app).selected_choice() {
        Some(ChoiceKind::RewindMessage { event_id, .. }) => event_id,
        _ => panic!("a rewind message is selected"),
    }
}

fn ids(app: &App) -> Vec<u64> {
    picker(app)
        .visible_choices()
        .iter()
        .filter_map(|choice| match choice.kind {
            ChoiceKind::RewindMessage { event_id, .. } => Some(event_id),
            _ => None,
        })
        .collect()
}

fn request(app: &mut App) -> (Selection, Ticket, u64) {
    let requests: Vec<_> = std::mem::take(&mut app.effects)
        .into_iter()
        .filter_map(|job| match job {
            Job::RewindHistory(selection, ticket, before) => Some((selection, ticket, before)),
            _ => None,
        })
        .collect();
    assert_eq!(1, requests.len(), "one bounded page request is issued");
    requests.into_iter().next().unwrap()
}

fn reply(app: &mut App, request: (Selection, Ticket, u64), result: Result<HistoryPage, Error>) {
    app.update(Update::RewindHistory(
        request.0, request.1, request.2, result,
    ));
}

fn source_request(app: &mut App) -> (Selection, Ticket, u64) {
    let requests: Vec<_> = std::mem::take(&mut app.effects)
        .into_iter()
        .filter_map(|job| match job {
            Job::RewindSource(selection, ticket, event_id) => Some((selection, ticket, event_id)),
            _ => None,
        })
        .collect();
    assert_eq!(1, requests.len(), "one original-message request is issued");
    requests.into_iter().next().unwrap()
}

fn source_reply(app: &mut App, request: (Selection, Ticket, u64), result: Result<String, Error>) {
    app.update(Update::RewindSource(
        request.0, request.1, request.2, result,
    ));
}

fn finish_source(app: &mut App, text: &str) {
    let request = source_request(app);
    source_reply(app, request, Ok(text.into()));
}

fn top(app: &mut App) {
    press(app, KeyCode::PageUp);
}

fn earliest(app: &mut App) {
    for _ in 0..MAX_RECORDS {
        top(app);
    }
}

fn retained_text_bytes(app: &App) -> usize {
    picker(app)
        .visible_choices()
        .iter()
        .map(|choice| match &choice.kind {
            ChoiceKind::RewindMessage { text, .. } => text.len(),
            _ => 0,
        })
        .sum()
}

fn message_text(app: &App, id: u64) -> &str {
    picker(app)
        .visible_choices()
        .into_iter()
        .find_map(|choice| match &choice.kind {
            ChoiceKind::RewindMessage { event_id, text, .. } if *event_id == id => {
                Some(text.as_str())
            }
            _ => None,
        })
        .expect("message remains available")
}

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn boundary_loads_one_page_and_preserves_selection_through_two_older_pages() {
    let mut app = app();
    assert_eq!(102, selected(&app));
    assert_eq!(vec![100, 101, 102], ids(&app));
    press(&mut app, KeyCode::Down);
    assert_eq!(102, selected(&app));
    press(&mut app, KeyCode::Up);
    assert_eq!(101, selected(&app));
    press(&mut app, KeyCode::Up);
    assert_eq!(100, selected(&app));
    for _ in 0..10 {
        press(&mut app, KeyCode::Up);
    }
    assert!(picker(&app).loading);
    assert!(render(&mut app).contains("Loading earlier messages"));
    let first = request(&mut app);
    assert_eq!(100, first.2);
    press(&mut app, KeyCode::Down);
    let mut older = page(&[80, 90, 90, 100], Some(80));
    let mut other_agent = entry(70);
    other_agent.agent = "reviewer".into();
    older.entries.push(other_agent);
    let mut assistant = entry(85);
    assistant.kind = HistoryKind::Assistant;
    older.entries.push(assistant);
    reply(&mut app, first, Ok(older));
    assert_eq!(101, selected(&app));
    assert_eq!(vec![80, 90, 100, 101, 102], ids(&app));
    assert!(!picker(&app).loading);
    assert!(
        app.effects.is_empty(),
        "a completed page does not drain further pages"
    );
    assert_eq!(3, app.pane().transcript.rewind_messages("main").len());

    top(&mut app);
    let second = request(&mut app);
    assert_eq!(80, second.2);
    reply(&mut app, second, Ok(page(&[20, 40, 60], None)));
    assert_eq!(80, selected(&app));
    assert_eq!(vec![20, 40, 60, 80, 90, 100, 101, 102], ids(&app));
    assert!(picker(&app).header.contains("All earlier messages loaded"));
    top(&mut app);
    assert_eq!(20, selected(&app));
    press(&mut app, KeyCode::Up);
    assert_eq!(20, selected(&app));
    press(&mut app, KeyCode::Down);
    assert_eq!(40, selected(&app));
    assert!(app.effects.is_empty());
}

#[test]
fn an_empty_selectable_page_advances_once_and_waits_for_another_up() {
    let mut app = app();
    top(&mut app);
    let first = request(&mut app);
    let mut older = page(&[50], Some(50));
    older.entries[0].kind = HistoryKind::Assistant;
    reply(&mut app, first, Ok(older));
    assert_eq!(vec![100, 101, 102], ids(&app));
    assert_eq!(100, selected(&app));
    assert!(app.effects.is_empty());
    press(&mut app, KeyCode::Up);
    assert_eq!(50, request(&mut app).2);
}

#[test]
fn an_empty_current_user_page_still_loads_the_previous_committed_message() {
    let mut app = connected_app();
    let mut current = page(&[100], Some(100));
    current.entries[0].kind = HistoryKind::Assistant;
    app.pane_mut().transcript.replace_recent(current);
    app.open_rewind_picker();
    assert!(ids(&app).is_empty());
    press(&mut app, KeyCode::Up);
    let first = request(&mut app);
    assert_eq!(100, first.2);
    reply(&mut app, first, Ok(page(&[80], None)));
    assert_eq!(80, selected(&app));
    press(&mut app, KeyCode::Enter);
    finish_source(&mut app, "message 80");
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(prompt.kind, PromptKind::RewindMessage { event_id: 80, .. })));
}

#[test]
fn a_bounded_message_window_keeps_the_selection_and_can_reach_the_oldest_page() {
    let mut app = connected_app();
    app.pane_mut()
        .transcript
        .replace_recent(page(&[1000, 1001, 1002], Some(1000)));
    app.open_rewind_picker();
    let mut before = 1000u64;
    for _ in 0..24 {
        earliest(&mut app);
        let anchor = selected(&app);
        let loading = request(&mut app);
        assert_eq!(before, loading.2);
        let first = before.saturating_sub(HISTORY_PAGE as u64).max(1);
        let entries: Vec<_> = (first..before).map(entry).collect();
        let next_before = (first > 1).then_some(first);
        reply(
            &mut app,
            loading,
            Ok(HistoryPage {
                entries,
                rewind_groups: Vec::new(),
                next_before,
            }),
        );
        assert_eq!(anchor, selected(&app));
        let visible = ids(&app);
        assert!(visible.len() <= MAX_RECORDS);
        assert!(retained_text_bytes(&app) <= MAX_RECORDS * MAX_BODY_BYTES);
        assert!(
            (first..before).all(|id| visible.contains(&id)),
            "every message from the new oldest page remains selectable"
        );
        before = first;
        if before == 1 {
            break;
        }
    }
    earliest(&mut app);
    assert_eq!(1, selected(&app));
    assert!(app.effects.is_empty());
    assert!(picker(&app)
        .header
        .contains("reopen rewind for recent messages"));
    press(&mut app, KeyCode::Enter);
    finish_source(&mut app, "message 1");
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(prompt.kind, PromptKind::RewindMessage { event_id: 1, .. })));
    press(&mut app, KeyCode::Esc);
    app.effects.clear();
    app.open_rewind_picker();
    assert_eq!(1002, selected(&app));
    assert_eq!(vec![1000, 1001, 1002], ids(&app));
}

#[test]
fn a_bounded_text_window_preserves_full_bodies_and_still_loads_earlier_messages() {
    let mut app = app();
    let mut before = 100;
    let mut oldest_body = String::new();
    for index in 0..12 {
        earliest(&mut app);
        let anchor = selected(&app);
        let loading = request(&mut app);
        assert_eq!(before, loading.2);
        let id = before - 1;
        let body = format!("full body {id}\n{}\nEND {id}", "x".repeat(900 * 1024));
        let mut message = entry(id);
        message.text = Some(body.clone());
        reply(
            &mut app,
            loading,
            Ok(HistoryPage {
                entries: vec![message],
                rewind_groups: Vec::new(),
                next_before: (index < 11).then_some(id),
            }),
        );
        assert_eq!(anchor, selected(&app));
        assert!(ids(&app).len() <= MAX_RECORDS);
        assert!(retained_text_bytes(&app) <= MAX_RECORDS * MAX_BODY_BYTES);
        assert!(
            message_text(&app, id) == body,
            "the new oldest message keeps its complete body"
        );
        before = id;
        oldest_body = body;
    }
    assert!(picker(&app).header.contains("History window limit reached"));
    earliest(&mut app);
    assert_eq!(before, selected(&app));
    assert!(app.effects.is_empty());
    press(&mut app, KeyCode::Enter);
    finish_source(&mut app, &oldest_body);
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::RewindMessage { event_id, text }
            if *event_id == before && text == &oldest_body)));
}

#[test]
fn an_oversized_history_item_uses_the_service_error_without_silently_truncating_it() {
    let mut app = app();
    top(&mut app);
    let first = request(&mut app);
    reply(
        &mut app,
        first,
        Err(Error::Service {
            status: 413,
            code: "history_entry_too_large".into(),
            error_message: "history item exceeds the response byte limit".into(),
        }),
    );
    assert_eq!(vec![100, 101, 102], ids(&app));
    assert!(!picker(&app).loading);
    assert!(picker(&app)
        .problem
        .as_ref()
        .unwrap()
        .contains("history_entry_too_large"));
    assert!(picker(&app).problem.as_ref().unwrap().contains("retries"));
    assert!(app.pending.is_empty());
}

#[test]
fn failed_pages_retry_with_a_new_ticket_and_old_retry_responses_are_ignored() {
    let mut app = app();
    top(&mut app);
    let first = request(&mut app);
    reply(
        &mut app,
        first.clone(),
        Err(Error::Protocol("fixture page unavailable".into())),
    );
    assert!(!picker(&app).loading);
    assert!(render(&mut app).contains("fixture page unavailable"));
    assert!(picker(&app).problem.as_ref().unwrap().contains("retries"));
    press(&mut app, KeyCode::Up);
    let retry = request(&mut app);
    assert_eq!(first.2, retry.2);
    assert_ne!(first.1, retry.1);
    reply(&mut app, first, Ok(page(&[1], None)));
    assert!(picker(&app).loading);
    assert_eq!(vec![100, 101, 102], ids(&app));
    reply(&mut app, retry, Ok(page(&[70], None)));
    assert_eq!(vec![70, 100, 101, 102], ids(&app));
    assert_eq!(100, selected(&app));
    assert!(picker(&app).problem.is_none());
}

#[test]
fn nonadvancing_pages_keep_the_cursor_available_for_explicit_retry() {
    let mut app = app();
    top(&mut app);
    let first = request(&mut app);
    reply(&mut app, first, Ok(page(&[99], Some(100))));
    assert_eq!(vec![100, 101, 102], ids(&app));
    assert!(picker(&app)
        .problem
        .as_ref()
        .unwrap()
        .contains("did not advance"));
    press(&mut app, KeyCode::Up);
    assert_eq!(100, request(&mut app).2);
}

#[test]
fn escape_preserves_the_draft_and_closed_or_reopened_picker_ignores_old_pages() {
    let mut app = app();
    app.pane_mut()
        .draft
        .set_text("first line\nsecond line".into());
    app.pane_mut().draft.left();
    let draft = app.pane().draft.text().to_owned();
    let cursor = app.pane().draft.cursor();
    top(&mut app);
    let first = request(&mut app);
    press(&mut app, KeyCode::Esc);
    reply(&mut app, first.clone(), Ok(page(&[1], None)));
    assert!(app.modal.is_none());
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
    assert!(app.pending.is_empty());
    assert!(!app
        .effects
        .iter()
        .any(|job| matches!(job, Job::Save(_) | Job::Send(_))));
    app.effects.clear();
    app.open_rewind_picker();
    top(&mut app);
    let current = request(&mut app);
    assert_ne!(first.1.id, current.1.id);
    app.choose(
        PickerKind::Rewind,
        ChoiceKind::RewindMessage {
            event_id: 1,
            turn: 1,
            text: "stale choice".into(),
        },
        first.1.id,
    );
    assert!(
        picker(&app).loading,
        "a stale choice cannot replace the reopened picker"
    );
    reply(&mut app, first, Ok(page(&[1], None)));
    assert!(picker(&app).loading);
    assert_eq!(vec![100, 101, 102], ids(&app));
    reply(&mut app, current, Ok(page(&[80], None)));
    assert_eq!(vec![80, 100, 101, 102], ids(&app));
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
}

#[test]
fn ordinary_reading_and_rewind_picker_pages_settle_independently() {
    let mut app = app();
    press(&mut app, KeyCode::Esc);
    app.effects.clear();
    app.scroll(-200);
    let (selection, ordinary) = std::mem::take(&mut app.effects)
        .into_iter()
        .find_map(|job| match job {
            Job::History(selection, request) => Some((selection, request)),
            _ => None,
        })
        .expect("reading requests its own earlier page");
    let anchor = app.pane().transcript.reading_anchor().cloned();
    assert!(anchor.is_some());
    app.open_rewind_picker();
    top(&mut app);
    let rewind = request(&mut app);
    app.update(Update::History(
        selection,
        ordinary,
        Ok(page(&[70, 80, 90], None)),
    ));
    assert!(
        picker(&app).loading,
        "ordinary history cannot settle the selector request"
    );
    assert_eq!(vec![100, 101, 102], ids(&app));
    assert_eq!(anchor.as_ref(), app.pane().transcript.reading_anchor());
    reply(&mut app, rewind, Ok(page(&[10], None)));
    assert_eq!(vec![10, 100, 101, 102], ids(&app));
    assert_eq!(anchor.as_ref(), app.pane().transcript.reading_anchor());
    assert_eq!(3, app.pane().transcript.rewind_messages("main").len());
}

#[test]
fn switching_agents_ignores_old_pages_and_prevents_rewinding_the_old_selection() {
    let mut app = app();
    top(&mut app);
    let first = request(&mut app);
    app.select(
        Address::new(app.active.session.clone(), "reviewer".into()),
        true,
        String::new(),
    );
    app.effects.clear();
    reply(&mut app, first, Ok(page(&[1], None)));
    assert_eq!(vec![100, 101, 102], ids(&app));
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    assert!(!app.effects.iter().any(|job| matches!(job, Job::Save(_))));
}

#[test]
fn an_older_loaded_message_enters_confirmation_and_commits_its_exact_event() {
    let mut app = app();
    top(&mut app);
    let first = request(&mut app);
    reply(&mut app, first, Ok(page(&[1, 10], None)));
    top(&mut app);
    assert_eq!(1, selected(&app));
    press(&mut app, KeyCode::Enter);
    finish_source(&mut app, "message 1");
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::RewindMessage { event_id: 1, text } if text == "message 1")
            && prompt.title.contains("Turn 1")
            && prompt.instructions.contains("[ ] Restore recorded file changes")));
    assert!(app.pending.is_empty());
    press(&mut app, KeyCode::Enter);
    assert!(app.effects.iter().any(|job| matches!(job, Job::Save(command)
        if matches!(&command.action, Action::RewindMessage { target_event_id: 1, text, restore_files: false } if text == "message 1"))));
    assert!(!app.effects.iter().any(|job| matches!(job, Job::Send(_))));
}

#[test]
fn a_current_display_preview_loads_the_original_before_confirming_and_saving() {
    let full = format!(
        "begin original\n{}\nEND ORIGINAL",
        "文".repeat(MAX_BODY_BYTES)
    );
    let mut original = entry(17);
    original.text = Some(full.clone());
    let mut app = connected_app();
    app.pane_mut().transcript.replace_recent(HistoryPage {
        entries: vec![original],
        rewind_groups: Vec::new(),
        next_before: None,
    });
    app.open_rewind_picker();
    assert!(message_text(&app, 17).len() < full.len());
    press(&mut app, KeyCode::Enter);
    for _ in 0..5 {
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Up);
    }
    assert!(picker(&app).loading);
    assert!(render(&mut app).contains("Loading original message"));
    assert!(app.pending.is_empty());
    let request = source_request(&mut app);
    assert_eq!(17, request.2);
    source_reply(&mut app, request, Ok(full.clone()));
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::RewindMessage { event_id: 17, text } if text == &full)));
    assert!(app.pending.is_empty());
    press(&mut app, KeyCode::Enter);
    assert!(app.effects.iter().any(|job| matches!(job, Job::Save(command)
        if matches!(&command.action, Action::RewindMessage { target_event_id: 17, text, restore_files: false } if text == &full))));
}

#[test]
fn source_escape_and_reopened_retry_preserve_drafts_and_ignore_old_results() {
    let mut app = app();
    app.pane_mut().draft.set_text("keep\nthis draft".into());
    app.pane_mut().draft.left();
    let draft = app.pane().draft.text().to_owned();
    let cursor = app.pane().draft.cursor();
    press(&mut app, KeyCode::Enter);
    assert_eq!(102, selected(&app));
    let first = source_request(&mut app);
    press(&mut app, KeyCode::Esc);
    source_reply(&mut app, first.clone(), Ok("ignored old source".into()));
    assert!(app.modal.is_none());
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
    app.effects.clear();
    app.open_rewind_picker();
    press(&mut app, KeyCode::Enter);
    let current = source_request(&mut app);
    source_reply(&mut app, first, Err(Error::Protocol("stale error".into())));
    assert!(picker(&app).loading);
    source_reply(
        &mut app,
        current.clone(),
        Err(Error::Protocol("source temporarily unavailable".into())),
    );
    assert!(!picker(&app).loading);
    assert_eq!(102, selected(&app));
    assert!(picker(&app)
        .problem
        .as_ref()
        .unwrap()
        .contains("Enter retries"));
    press(&mut app, KeyCode::Enter);
    let retry = source_request(&mut app);
    assert_ne!(current.1, retry.1);
    source_reply(&mut app, current, Ok("stale retry source".into()));
    assert!(picker(&app).loading);
    source_reply(&mut app, retry, Ok("message 102".into()));
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(&prompt.kind, PromptKind::RewindMessage { text, .. } if text == "message 102")));
    press(&mut app, KeyCode::Esc);
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
    assert!(app.pending.is_empty());
}

#[test]
fn original_source_supersedes_page_reads_and_requires_the_current_event() {
    let mut app = app();
    top(&mut app);
    let page_request = request(&mut app);
    press(&mut app, KeyCode::Enter);
    let source = source_request(&mut app);
    assert_eq!(100, source.2);
    reply(&mut app, page_request, Ok(page(&[1], None)));
    assert!(picker(&app).loading);
    let mut wrong_event = source.clone();
    wrong_event.2 = 99;
    source_reply(&mut app, wrong_event, Ok("wrong event".into()));
    assert!(picker(&app).loading);
    source_reply(&mut app, source, Ok("message 100".into()));
    assert!(matches!(&app.modal, Some(Modal::Prompt(prompt))
        if matches!(prompt.kind, PromptKind::RewindMessage { event_id: 100, .. })));
}

#[test]
fn original_source_from_a_previous_agent_cannot_open_confirmation() {
    let mut app = app();
    press(&mut app, KeyCode::Enter);
    let source = source_request(&mut app);
    app.select(
        Address::new(app.active.session.clone(), "reviewer".into()),
        true,
        String::new(),
    );
    app.effects.clear();
    source_reply(&mut app, source, Ok("old agent source".into()));
    assert!(matches!(&app.modal, Some(Modal::Picker(_))));
    press(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    assert!(app.pending.is_empty());
    assert!(!app.effects.iter().any(|job| matches!(job, Job::Save(_))));
}
