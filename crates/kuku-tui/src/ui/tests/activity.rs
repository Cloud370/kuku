use std::time::{Duration, Instant, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::{
    Acknowledgement, Action, AgentProgress, Error, HistoryPage, Observation, Receipt,
};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

use super::super::banner;
use super::{activity_screen, app, key, screen, snapshot};
use crate::app::{Address, App};
use crate::config::StatusLineSettings;
use crate::jobs::{Job, Update};
use crate::statusline::StatusLine;
use crate::ui::{ActivityClock, render_at};

#[test]
fn composer_counts_running_jobs_when_the_agent_is_idle() {
    let mut app = app();
    app.select(app.active.clone(), true, "Jobs".into());
    let selection = app.selection();
    let mut state = snapshot(&app, "idle", json!(null), json!(null));
    state.agent.running_jobs = 2;
    app.update(Update::Loaded(
        selection.clone(),
        state,
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));
    let (rendered, _) = activity_screen(&mut app, 92, 24, None);
    assert!(rendered.contains("2 jobs · /jobs"));
}

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
            rewind_groups: Vec::new(),
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
    assert!(!first_heading.contains("Send to"));
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
    assert!(
        screen(&mut app, 92, 24)
            .lines()
            .nth(19)
            .unwrap()
            .contains("Offline")
    );
}

#[test]
fn attached_activity_keeps_clock_tokens_and_draft_fixed_across_phases() {
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
            rewind_groups: Vec::new(),
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
            heading.contains(label) && !heading.contains("Send to"),
            "{heading}"
        );
        let footer: String = row(27).iter().map(|cell| cell.symbol()).collect();
        let footer_label = match label {
            "Model tool arguments" => "Writing tool arguments",
            other => other,
        };
        assert!(
            footer
                .trim()
                .ends_with(&format!("{footer_label} · Agent main")),
            "{footer}"
        );
        (
            heading,
            terminal.get_cursor_position().unwrap(),
            app.composer_area,
        )
    };
    let original = draw(&mut app, &mut clock, 0, "Waiting for model");
    assert!(original.0.contains("Turn 8s"));
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
    let tools = draw(&mut app, &mut clock, 2_100, "Model tool arguments");
    assert!(tools.0.contains("~3 tok"));
    assert_eq!(original.0.find("Draft"), tools.0.find("Draft"));
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
        recipient.contains("To review")
            && recipient.contains("-92 ▾")
            && recipient.contains("Ready"),
        "{recipient}"
    );
    assert!(
        rows.lines()
            .nth(21)
            .unwrap()
            .trim_matches([' ', '│'])
            .is_empty()
    );
}

#[test]
fn selected_recipient_stays_visible_at_the_top_and_beside_the_draft() {
    let mut app = app();
    app.statusline_settings = StatusLineSettings::in_memory(StatusLine::parse("model").unwrap());
    let main = app.active.clone();
    app.select(main.clone(), true, "Shared title".into());
    let reviewer = Address::new(main.session.clone(), "reviewer".into());
    app.select(reviewer.clone(), true, "Shared title".into());

    for width in [32, 80, 128] {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        let mut recipient_start = None;
        for (address, accent, background) in [
            (
                reviewer.clone(),
                crate::theme::MODEL,
                crate::theme::SUBAGENT_SURFACE,
            ),
            (main.clone(), crate::theme::ACCENT, crate::theme::SURFACE),
        ] {
            app.select(address.clone(), true, "Shared title".into());
            let buffer = terminal
                .draw(|frame| super::super::render(frame, &mut app))
                .unwrap()
                .buffer;
            let row = |y: u16| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            };
            assert!(
                row(1).contains(&format!("Agent {}", address.agent)),
                "{}",
                row(1)
            );
            let to = app.recipient_area;
            let heading = row(to.y);
            let phase = buffer.cell((app.composer_area.x, to.y)).unwrap();
            assert!(
                crate::theme::KUKU_MARK.starts_with(phase.symbol()),
                "{heading}"
            );
            assert!(to.x > app.composer_area.x);
            assert_eq!(to.x, *recipient_start.get_or_insert(to.x));
            let label: String = (to.x..to.right())
                .map(|x| buffer.cell((x, to.y)).unwrap().symbol())
                .collect();
            assert!(
                label == format!("Send to {} ▾", address.agent)
                    || label == format!("To {} ▾", address.agent)
                    || (width == 32 && label.starts_with("To rev") && label.ends_with("… ▾")),
                "{label:?} in {heading}"
            );
            assert_eq!(buffer.cell((to.x, to.y)).unwrap().fg, accent);
            assert_eq!(buffer.cell((to.right() - 1, to.y)).unwrap().symbol(), "▾");
            let session = crate::theme::session_area(buffer.area);
            assert_eq!(buffer.cell((session.left(), to.y)).unwrap().fg, accent);
            let right = session.right() - 1;
            assert_eq!(buffer.cell((right, to.y)).unwrap().bg, background);
            assert_eq!(
                buffer.cell((right, app.composer_area.y)).unwrap().bg,
                background
            );
        }
    }
}

#[test]
fn compact_heading_shows_only_real_parallel_tools_and_prefers_reported_output() {
    let mut app = app();
    app.select(app.active.clone(), true, "Live output".into());
    let selection = app.selection();
    let mut current = snapshot(&app, "running", json!(null), json!(null));
    current.progress = Some(AgentProgress::RunningTools {
        tool_call_ids: vec!["tool-1".into()],
    });
    app.update(Update::Loaded(
        selection.clone(),
        current,
        HistoryPage {
            entries: vec![],
            rewind_groups: vec![],
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection.clone()));
    let emit = |app: &mut App, event| {
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
    };
    let before = screen(&mut app, 60, 28);
    assert!(
        before.contains("Running tools") || before.contains("Tools running"),
        "{before}"
    );
    assert!(!before.contains("↳1"));
    emit(
        &mut app,
        json!({"type":"text","content":"01234567890123456789"}),
    );
    let estimate = screen(&mut app, 60, 28);
    assert!(
        estimate
            .lines()
            .any(|line| line.contains("Reply") && line.contains("↳1") && line.contains("~5 tok")),
        "{estimate}"
    );
    emit(
        &mut app,
        json!({"type":"model_output_usage","output_tokens":37}),
    );
    let reported = screen(&mut app, 60, 28);
    assert!(
        reported
            .lines()
            .any(|line| line.contains("Reply") && line.contains("↳1") && line.contains("37 tok")),
        "{reported}"
    );
    assert!(!reported.contains("~5 tok"));
}

#[test]
fn retry_warning_keeps_stop_action_visible_with_narrow_unicode_text() {
    let mut app = app();
    app.select(app.active.clone(), true, "Retry notices".into());
    let selection = app.selection();
    let message = "服务暂时不可用，请检查网络连接并等待重试。".repeat(30);
    let mut current = snapshot(&app, "running", json!(null), json!(null));
    current.progress = Some(
        serde_json::from_value(json!({
            "phase":"retrying_model",
            "retry":{
                "source":"runtime.model_retry","phase":"waiting","attempt":2,
                "max_attempts":4,"failed_kind":"provider_unavailable",
                "delay_ms":5000,"ts":"9999-12-31T23:59:59Z",
                "message":message,"will_retry":true
            }
        }))
        .unwrap(),
    );
    app.update(Update::Loaded(
        selection.clone(),
        current,
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));

    for width in [52, 80] {
        let screen = activity_screen(&mut app, width, 60, Some(0)).0;
        let compact = screen.replace(' ', "");
        assert!(screen.contains("Retry in 5s"), "{screen}");
        assert!(screen.contains("attempt 2/4"), "{screen}");
        assert!(screen.contains("Esc stop"), "{screen}");
        assert!(compact.contains("服务暂时不可用"), "{screen}");
    }
}
#[test]
fn stopped_execution_notice_is_rendered_above_the_composer() {
    let mut app = app();
    app.select(app.active.clone(), true, "Stopped execution".into());
    let selection = app.selection();
    app.update(Update::Loaded(
        selection.clone(),
        snapshot(&app, "running", json!(null), json!(null)),
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));
    app.interrupt();
    let command = app.effects.iter().find_map(|job| match job {
        Job::Save(command) if matches!(&command.action, Action::Cancel { execution_id: 49 }) => {
            Some(command.clone())
        }
        _ => None,
    });
    let command = command.expect("running execution should save its cancel command");
    app.update(Update::Sent(
        command.command_id.clone(),
        Ok(Acknowledgement {
            receipt: Receipt {
                command_id: command.command_id,
                event_id: 53,
                agent: "main".into(),
            },
            replayed: false,
            status: None,
            warnings: vec![],
        }),
    ));

    let rendered = screen(&mut app, 92, 24);
    assert!(
        rendered.contains("Execution stopped · draft kept"),
        "{rendered}"
    );
}

#[test]
fn stopped_retry_footer_does_not_repeat_transcript_details() {
    let mut app = app();
    app.select(app.active.clone(), true, "Stopped retry".into());
    let selection = app.selection();
    let mut current = snapshot(&app, "interrupted", json!(null), json!(null));
    current.agent.last_error = Some("Fixture connection closed".into());
    current.progress = Some(
        serde_json::from_value(json!({
            "phase":"retrying_model",
            "retry":{
                "source":"runtime.model_retry",
                "phase":"stopped",
                "attempt":2,
                "max_attempts":3,
                "request_id":"req_1",
                "failed_kind":"transport",
                "status":502,
                "will_retry":false,
                "stop_reason":"stream_started",
                "message":"Fixture connection closed"
            }
        }))
        .unwrap(),
    );
    app.update(Update::Loaded(
        selection.clone(),
        current,
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));

    let lines = banner(&app, 80, 3);
    assert_eq!(1, lines.len());
    let footer = lines[0].to_string();
    assert_eq!(
        "Automatic retry stopped; see transcript for details.",
        footer
    );
    assert!(!footer.contains("Fixture connection closed"));
}
