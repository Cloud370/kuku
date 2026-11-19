use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::{Action, HistoryEntry, HistoryKind, HistoryPage};
use serde_json::json;

use super::{app, key, snapshot};
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
        Job::Save(command) if matches!(command.action, Action::Cancel { .. })
    )));
    assert!(running.escape_armed_at.is_none());

    key(&mut running, KeyCode::Esc, KeyModifiers::NONE);
    assert!(running.effects.iter().any(|job| matches!(job,
        Job::Save(command) if matches!(command.action, Action::Cancel { execution_id: 49 })
    )));

    let mut idle = app();
    load_snapshot(&mut idle, "idle");
    let idle_notes = open_thinking_notes(&mut idle);
    idle.effects.clear();
    key(&mut idle, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!idle.pane().transcript.note_is_open(&idle_notes));
    assert!(idle.effects.is_empty());
    assert!(idle.escape_armed_at.is_none());
}
