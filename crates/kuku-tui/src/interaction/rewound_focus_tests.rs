use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kuku_client::{HistoryEntry, HistoryKind, HistoryPage, RewindGroup, RewindGroupPage};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::app::App;
use crate::jobs::{Job, Update};

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
fn expanding_rewind_group_focuses_history_and_escape_restores_multiline_draft() {
    let mut app = connected_app();
    let entry = |id, kind, text: &str| HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(1),
        kind,
        text: Some(text.into()),
        turn_summary: None,
        details: Default::default(),
    };
    app.pane_mut().transcript.replace_recent(HistoryPage {
        entries: vec![
            entry(1, HistoryKind::User, "earlier active entry"),
            entry(100, HistoryKind::Assistant, "active tail"),
        ],
        rewind_groups: vec![RewindGroup {
            rewind_event_id: 50,
            target_event_id: 2,
            agent: "main".into(),
            turn_number: Some(1),
            entry_count: 7,
        }],
        next_before: None,
    });
    app.paste("first line\nsecond line");
    let draft = app.pane().draft.text().to_owned();
    let cursor = app.pane().draft.cursor();

    assert!(app.toggle_readonly_rewind_group(50));
    press(&mut app, KeyCode::Enter);
    let request = app
        .effects
        .iter()
        .find_map(|job| match job {
            Job::RewindGroupHistory(_, request) => Some(request.clone()),
            _ => None,
        })
        .expect("expanding the group requests its page");
    app.pane_mut().transcript.apply_rewind_group_page(
        &request,
        Ok(RewindGroupPage {
            entries: (3..10)
                .map(|id| {
                    entry(
                        id,
                        HistoryKind::User,
                        &format!("hidden entry {id}\n{}", "older content ".repeat(8)),
                    )
                })
                .collect(),
            next_before: None,
        }),
    );
    app.effects.clear();

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    assert!(!app.composer_focused());
    let anchor = app.pane().transcript.reading_anchor().cloned().unwrap();
    press(&mut app, KeyCode::Up);
    assert_eq!(cursor, app.pane().draft.cursor());
    assert_ne!(Some(&anchor), app.pane().transcript.reading_anchor());

    press(&mut app, KeyCode::Esc);
    assert!(app.composer_focused());
    assert_eq!(draft, app.pane().draft.text());
    assert_eq!(cursor, app.pane().draft.cursor());
    assert!(app.effects.is_empty());

    assert!(app.toggle_readonly_rewind_group(50));
    press(&mut app, KeyCode::Enter);
    assert!(
        app.composer_focused(),
        "collapsing should not steal composer focus"
    );
    assert_eq!(draft, app.pane().draft.text());
}
