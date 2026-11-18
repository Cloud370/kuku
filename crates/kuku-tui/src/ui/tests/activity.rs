use std::time::{Duration, Instant, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::{AgentProgress, Error, HistoryPage, Observation};
use ratatui::style::Modifier;
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;

use super::{activity_screen, app, key, screen, snapshot};
use crate::app::{Address, App};
use crate::config::StatusLineSettings;
use crate::jobs::Update;
use crate::statusline::StatusLine;
use crate::ui::{render_at, ActivityClock};

#[test]
fn composer_animates_only_observed_work_and_keeps_feedback_in_place() {
    let mut app = app();
    app.select(app.active.clone(), true, "Motion".into());
    let selection = app.selection();
    app.update(Update::Loaded(
        selection.clone(),
        snapshot(&app, "running", json!(null), json!(null)),
        HistoryPage {
            entries: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));
    app.pane_mut().draft.insert("local draft");
    let first = activity_screen(&mut app, 92, 24, Some(0));
    let next = activity_screen(&mut app, 92, 24, Some(3));
    let first_heading = first.0.lines().nth(19).unwrap();
    let next_heading = next.0.lines().nth(19).unwrap();
    assert!(first.1 && next.1);
    assert_ne!(first_heading, next_heading);
    assert_eq!(first_heading.find("To main"), next_heading.find("To main"));
    assert_eq!(first_heading.find("Running"), next_heading.find("Running"));
    assert_eq!(first_heading.find("Draft"), next_heading.find("Draft"));
    assert_eq!(first.0.lines().last(), next.0.lines().last());
    assert!(!first.0.lines().last().unwrap().contains("Running"));

    key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
    let hidden = activity_screen(&mut app, 92, 24, Some(5));
    assert!(!hidden.1);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.update(Update::ConnectFailed(Error::Options(
        "fixture offline".into(),
    )));
    assert_eq!(
        activity_screen(&mut app, 92, 24, Some(1)).0,
        activity_screen(&mut app, 92, 24, Some(6)).0,
    );
    assert!(screen(&mut app, 92, 24)
        .lines()
        .nth(19)
        .unwrap()
        .contains("Offline"));
}

#[test]
fn attached_activity_keeps_recipient_clock_and_draft_fixed_across_phases() {
    let mut app = app();
    app.statusline_settings =
        StatusLineSettings::in_memory(StatusLine::parse("model,execution,agent").unwrap());
    app.select(app.active.clone(), true, "In-progress Turn".into());
    let selection = app.selection();
    let mut current = snapshot(&app, "running", json!(null), json!(null));
    current.agent.turn = Some(40);
    current.agent.turn_started_at_ms = Some(10_000);
    current.progress = Some(AgentProgress::WaitingForModel);
    app.update(Update::Loaded(
        selection.clone(),
        current,
        HistoryPage {
            entries: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection.clone()));
    app.pane_mut()
        .draft
        .set_text("First draft row\nDraft with 中文".into());

    let width = 128;
    let mut terminal = Terminal::new(TestBackend::new(width, 28)).unwrap();
    let first = Instant::now();
    let wall = UNIX_EPOCH + Duration::from_secs(18);
    let mut clock = ActivityClock::default();
    let mut draw = |app: &mut App, clock: &mut ActivityClock, ms, label: &str| {
        let passed = Duration::from_millis(ms);
        let activity = clock.sample(app, first + passed, wall + passed);
        terminal
            .draw(|frame| {
                render_at(frame, app, activity);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y| &buffer.content[usize::from(y * width)..usize::from((y + 1) * width)];
        let cells = row(app.composer_area.y - 2);
        let heading: String = cells.iter().map(|cell| cell.symbol()).collect();
        assert!(
            heading.contains("To main") && heading.contains(label),
            "{heading}"
        );
        assert!(cells.windows(4).any(|name| name
            .iter()
            .map(|cell| cell.symbol())
            .eq(["m", "a", "i", "n"])
            && name
                .iter()
                .all(|cell| cell.modifier.contains(Modifier::BOLD))));
        let footer: String = row(27).iter().map(|cell| cell.symbol()).collect();
        assert!(
            footer.trim().ends_with(&format!("{label} · Agent main")),
            "{footer}"
        );
        (
            heading,
            terminal.get_cursor_position().unwrap(),
            app.composer_area,
        )
    };
    let original = draw(&mut app, &mut clock, 0, "Waiting for model");
    assert!(original.0.contains("8s"));
    let live = |app: &mut App, clock: &mut ActivityClock, ms, event| {
        app.update(Update::Observation(
            selection.clone(),
            Observation::Live {
                agent: "main".into(),
                execution_id: 49,
                cursor: 53,
                turn: Some(40),
                turn_number: Some(1),
                event,
            },
        ));
        clock.observe(app, first + Duration::from_millis(ms));
    };
    live(&mut app, &mut clock, 40, json!({"type":"thinking"}));
    draw(&mut app, &mut clock, 45, "Waiting for model");
    live(
        &mut app,
        &mut clock,
        70,
        json!({"type":"text","content":"Early text"}),
    );
    live(&mut app, &mut clock, 95, json!({"type":"thinking"}));
    let current = draw(&mut app, &mut clock, 145, "Waiting for model");
    assert_eq!((original.1, original.2), (current.1, current.2));
    live(&mut app, &mut clock, 160, json!({"type":"tool_arguments"}));
    let tools = draw(&mut app, &mut clock, 2_100, "Writing tool arguments");
    assert!(tools.0.contains("10s"));
    assert_eq!(original.0.find("8s"), tools.0.find("10s"));
    assert_eq!((original.1, original.2), (tools.1, tools.2));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let stopped = draw(&mut app, &mut clock, 2_101, "Stopping");
    assert_eq!((original.1, original.2), (stopped.1, stopped.2));
    assert_eq!("First draft row\nDraft with 中文", app.pane().draft.text());
}

#[test]
fn narrow_composer_preserves_the_distinguishing_end_of_the_agent_address() {
    let mut app = app();
    app.select(
        Address::new(
            app.active.session.clone(),
            "reviewer-a-much-longer-name-92".into(),
        ),
        false,
        String::new(),
    );
    let rows = screen(&mut app, 71, 25);
    let recipient = rows.lines().nth(20).unwrap();
    assert!(
        recipient.contains("To reviewer") && recipient.contains("-92 · Ready"),
        "{recipient}"
    );
    assert!(rows
        .lines()
        .nth(21)
        .unwrap()
        .trim_matches([' ', '│'])
        .is_empty());
}
