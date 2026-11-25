use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::{Action, HistoryEntry, HistoryKind, HistoryPage};
use serde_json::json;

use super::{app, key, screen, snapshot};
use crate::app::App;
use crate::jobs::{Job, Update};

fn load_snapshot(app: &mut App, status: &str) {
    let state = snapshot(app, status, json!(null), json!(null));
    app.update(Update::Loaded(
        app.selection(),
        state,
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.effects.clear();
}

fn open_thinking_notes(app: &mut App) -> crate::transcript::DetailKey {
    let agent = app.active.agent.clone();
    app.pane_mut().transcript.apply(
        HistoryEntry {
            id: 11,
            agent,
            turn: Some(1),
            kind: HistoryKind::Assistant,
            text: Some("answer".into()),
            turn_summary: None,
            details: [("reasoning_preview".into(), json!("published thought"))]
                .into_iter()
                .collect(),
        },
        None,
    );
    app.pane_mut().transcript.project(82, 20);
    let key = app.pane().transcript.latest_notes().unwrap();
    assert!(app.toggle_transcript_detail(&key));
    key
}

#[test]
fn escape_closes_thinking_notes_before_running_interrupt_or_idle_rewind() {
    let mut running = app();
    load_snapshot(&mut running, "running");
    let running_notes = open_thinking_notes(&mut running);
    running.effects.clear();
    key(&mut running, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!running.pane().transcript.note_is_open(&running_notes));
    assert!(!running.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Cancel { .. } | Action::Interrupt { .. })
    )));
    assert!(!running.rewind_armed);

    key(&mut running, KeyCode::Esc, KeyModifiers::NONE);
    assert!(running.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Interrupt { execution_id: 49 })
    )));

    let mut idle = app();
    load_snapshot(&mut idle, "idle");
    let idle_notes = open_thinking_notes(&mut idle);
    idle.effects.clear();
    key(&mut idle, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!idle.pane().transcript.note_is_open(&idle_notes));
    assert!(idle.effects.is_empty());
    assert!(!idle.rewind_armed);
}

#[test]
fn history_metrics_dock_tracks_the_actual_visible_summary_row_across_scroll_and_resize() {
    let mut app = app();
    load_snapshot(&mut app, "idle");
    let first = (1..=28)
        .map(|n| format!("First reply paragraph {n:02}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    for value in [
        json!({"id":1,"agent":"main","type":"user","turn":1,"turn_number":1,"text":"First task"}),
        json!({"id":2,"agent":"main","type":"assistant","turn":1,"turn_number":1,"text":first}),
        json!({"id":3,"agent":"main","type":"turn_summary","turn":1,"turn_number":1,"turn_summary":{"tool_calls":17}}),
        json!({"id":4,"agent":"main","type":"user","turn":2,"turn_number":2,"text":"Second task"}),
        json!({"id":5,"agent":"main","type":"assistant","turn":2,"turn_number":2,"text":"Second reply"}),
        json!({"id":6,"agent":"main","type":"turn_summary","turn":2,"turn_number":2,"turn_summary":{"tool_calls":29}}),
    ] {
        app.pane_mut()
            .transcript
            .apply(serde_json::from_value(value).unwrap(), None);
    }
    let history_row = |painted: &str| {
        painted
            .lines()
            .find(|row| row.contains("Reading history"))
            .expect("history hint stays available")
            .to_owned()
    };
    let initial = screen(&mut app, 128, 90);
    assert_eq!(1, initial.matches("17 tools").count());
    assert_eq!(1, initial.matches("29 tools").count());
    assert!(!initial.contains("Reading history"));

    // Both turns fit, but scrolling up explicitly selects history mode.
    app.scroll(-100);
    let at_top = app.pane().transcript.reading_anchor().cloned();
    let short = screen(&mut app, 128, 14);
    assert!(history_row(&short).contains("17 tools"), "{short}");
    assert_eq!(1, short.matches("17 tools").count());
    assert!(!short.contains("29 tools"));
    let tall = screen(&mut app, 128, 90);
    assert!(!history_row(&tall).contains("17 tools"), "{tall}");
    assert_eq!(1, tall.matches("17 tools").count());
    assert_eq!(1, tall.matches("29 tools").count());
    assert_eq!(at_top.as_ref(), app.pane().transcript.reading_anchor());
    assert_eq!(short, screen(&mut app, 128, 14));
    assert_eq!(at_top.as_ref(), app.pane().transcript.reading_anchor());

    // Cross the exact row boundary, including when the stats row would have
    // been hidden underneath the overlaid dock on this short screen.
    let mut visible_footer = false;
    for _ in 0..80 {
        key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        let painted = screen(&mut app, 128, 14);
        assert!(app.pane().transcript.is_reading());
        assert_eq!(1, painted.matches("17 tools").count(), "{painted}");
        if !history_row(&painted).contains("17 tools") {
            visible_footer = true;
            break;
        }
    }
    assert!(
        visible_footer,
        "scroll should bring the first summary into view"
    );
    let footer_anchor = app.pane().transcript.reading_anchor().cloned();
    assert!(!history_row(&screen(&mut app, 128, 90)).contains("17 tools"));
    assert_eq!(
        footer_anchor.as_ref(),
        app.pane().transcript.reading_anchor()
    );
    assert!(!history_row(&screen(&mut app, 128, 14)).contains("17 tools"));
    assert_eq!(
        footer_anchor.as_ref(),
        app.pane().transcript.reading_anchor()
    );
    key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    let dock_again = screen(&mut app, 128, 14);
    assert!(
        history_row(&dock_again).contains("17 tools"),
        "{dock_again}"
    );
    assert_eq!(1, dock_again.matches("17 tools").count());

    key(&mut app, KeyCode::End, KeyModifiers::CONTROL);
    let latest = screen(&mut app, 128, 90);
    assert!(!app.pane().transcript.is_reading());
    assert!(!latest.contains("Reading history"));
    assert_eq!(1, latest.matches("17 tools").count());
    assert_eq!(1, latest.matches("29 tools").count());
}
