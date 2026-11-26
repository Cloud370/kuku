use std::time::{Duration, Instant, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyModifiers};
use kuku_client::{
    Acknowledgement, Action, AgentProgress, Error, HistoryPage, Input, InputSource, Observation,
    QueuedInput, Receipt,
};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

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
            "Preparing tool call" | "Preparing file contents" => "Writing tool arguments",
            "Replying" => "Streaming reply",
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
    draw(&mut app, &mut clock, 45, "Thinking");
    live(
        &mut app,
        &mut clock,
        70,
        json!({"type":"text","content":"Early text"}),
    );
    live(&mut app, &mut clock, 95, json!({"type":"thinking"}));
    let current = draw(&mut app, &mut clock, 145, "Thinking");
    assert_eq!((original.1, original.2), (current.1, current.2));
    live(&mut app, &mut clock, 160, json!({"type":"tool_arguments"}));
    let tools = draw(&mut app, &mut clock, 2_100, "Preparing tool call");
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
fn busy_recipient_send_keeps_the_only_sending_indicator_visible_at_narrow_width() {
    let mut app = app();
    let recipient = Address::new(app.active.session.clone(), "reviewer".into());
    app.select(recipient, true, "Shared title".into());
    let selection = app.selection();
    let mut state = snapshot(&app, "running", json!(null), json!(null));
    state.progress = Some(AgentProgress::WaitingForModel);
    app.update(Update::Loaded(
        selection.clone(),
        state,
        HistoryPage {
            entries: vec![],
            rewind_groups: vec![],
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));
    app.effects.clear();
    app.pane_mut().draft.insert("delayed busy send");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let Some(Job::Save(command)) = app.effects.pop() else {
        panic!("save before sending")
    };
    for saved in [false, true] {
        if saved {
            app.update(Update::Saved(command.clone(), Ok(())));
        }
        let rendered = screen(&mut app, 36, 24);
        assert!(rendered.contains("Sending"), "{rendered}");
        assert!(
            rendered.contains("Model wait") && rendered.contains("Agent reviewer"),
            "{rendered}"
        );
        assert!(
            !rendered.contains("delayed busy send")
                && !rendered.contains("Pending inputs")
                && !rendered.contains("Unconfirmed send"),
            "{rendered}"
        );
        assert!(!app.recipient_area.is_empty(), "{rendered}");
    }
}

#[test]
fn observed_new_execution_enables_escape_and_stop_before_snapshot_refresh() {
    use crate::contextual::ContextControl;

    for explicit_stop in [false, true] {
        let mut app = app();
        app.select(app.active.clone(), true, "Observe a new execution".into());
        let selection = app.selection();
        let mut old = snapshot(&app, "idle", json!(null), json!(null));
        old.agent.execution_id = None;
        app.update(Update::Loaded(
            selection.clone(),
            old.clone(),
            HistoryPage {
                entries: vec![],
                rewind_groups: vec![],
                next_before: None,
            },
        ));
        app.update(Update::WatchConnected(selection.clone()));
        // A live frame can precede its durable commit, and an outstanding
        // HTTP refresh can still return the old idle boundary.
        app.update(Update::Observation(
            selection.clone(),
            Observation::Live {
                agent: app.active.agent.clone(),
                execution_id: 53,
                cursor: 54,
                turn: Some(54),
                turn_number: Some(1),
                event: json!({"type":"text","content":"New execution"}),
            },
        ));
        app.update(Update::Refreshed(selection.clone(), Ok(old)));
        assert_eq!(Some(53), app.pane().running_execution());
        assert!(matches!(
            app.contextual_controls().first(),
            Some(ContextControl::Interrupt {
                execution_id: 53,
                ..
            })
        ));
        app.update(Update::Observation(
            selection,
            serde_json::from_value(json!({
                "type":"committed", "agent":"main", "id":53, "kind":"runtime.operation",
                "operation":"execution_started", "state_changed":true
            }))
            .unwrap(),
        ));
        app.effects.clear();
        app.pane_mut().draft.insert("unsent draft");
        key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
        assert!(app.pane().contextual_focus.active);
        if explicit_stop {
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        } else {
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.effects.is_empty());
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        }
        let Some(Job::Save(command)) = app.effects.pop() else {
            panic!("control addresses observed execution")
        };
        assert_eq!(
            command.action,
            if explicit_stop {
                Action::Cancel { execution_id: 53 }
            } else {
                Action::Interrupt { execution_id: 53 }
            }
        );
        assert_eq!("unsent draft", app.pane().draft.text());
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
    assert!(before.contains("Using tools"), "{before}");
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
fn interrupted_retry_explains_the_pause_and_the_composer_only_queues() {
    let mut app = app();
    app.select(app.active.clone(), true, "Stopped retry".into());
    let selection = app.selection();
    let mut current = snapshot(&app, "interrupted", json!(null), json!(null));
    current.agent.execution_id = None;
    current.agent.last_error = Some("provider error: SAFETY".into());
    current.agent.pending_inputs = 1;
    current.pending.push(QueuedInput {
        input: Input {
            source: InputSource::Host("http".into()),
            ..Input::new(
                "queued-command",
                app.active.agent.clone(),
                "Waiting to send",
            )
        },
        receipt: Receipt {
            command_id: "queued-command".into(),
            event_id: 51,
            agent: app.active.agent.clone(),
        },
    });
    current.progress = Some(
        serde_json::from_value(json!({
            "phase":"retrying_model",
            "retry":{
                "source":"runtime.model_retry",
                "phase":"stopped",
                "attempt":2,
                "max_attempts":3,
                "request_id":"req_1",
                "failed_kind":"provider_unavailable",
                "will_retry":false,
                "stop_reason":"stream_started",
                "message":"provider unavailable"
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

    let rendered = screen(&mut app, 80, 24);
    assert!(
        rendered.contains("1 saved · Paused · Continue session"),
        "{rendered}"
    );
    assert!(rendered.contains("provider error: SAFETY"), "{rendered}");
    assert!(!rendered.contains("1 queued"), "{rendered}");
    assert_eq!(
        rendered.matches("Continue session").count(),
        1,
        "{rendered}"
    );
    assert!(rendered.contains("Waiting to send"), "{rendered}");
    assert!(rendered.contains("Enter queue only"), "{rendered}");
    assert!(!rendered.contains("Enter steer"), "{rendered}");
    let narrow = screen(&mut app, 34, 20);
    assert!(narrow.contains("Continue session"), "{narrow}");
    assert!(narrow.contains("1 saved"), "{narrow}");

    let advancing = |app: &App| {
        app.effects.iter().any(
            |job| matches!(job, Job::Save(command) if matches!(command.action, Action::Advance)),
        )
    };
    assert!(!advancing(&app));
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(advancing(&app));
}

#[test]
fn activity_uses_dispatched_call_identity_and_clears_targets_on_attention_and_completion() {
    use super::super::activity::render_heading;

    let mut app = app();
    app.select(app.active.clone(), true, "Real tool phases".into());
    let selection = app.selection();
    let mut state = snapshot(&app, "running", json!(null), json!(null));
    state.agent.turn = Some(40);
    state.agent.running_jobs = 2;
    state.progress = Some(AgentProgress::WaitingForModel);
    app.update(Update::Loaded(
        selection.clone(),
        state,
        HistoryPage {
            entries: vec![],
            rewind_groups: vec![],
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));
    let start = Instant::now();
    let wall = UNIX_EPOCH + Duration::from_secs(100);
    let mut clock = ActivityClock::default();
    let heading = |app: &App, clock: &mut ActivityClock, ms, width| {
        let duration = Duration::from_millis(ms);
        let frame = clock.sample(app, start + duration, wall + duration);
        let line = render_heading(app, width, frame, false, false).0;
        assert!(line.width() <= width);
        line.to_string()
    };
    let live = |app: &mut App, event, cursor| {
        app.update(Update::Observation(
            app.selection(),
            Observation::Live {
                agent: "main".into(),
                execution_id: 49,
                cursor,
                turn: Some(40),
                turn_number: Some(1),
                event,
            },
        ))
    };
    let commit = |app: &mut App, id, kind, fields: serde_json::Value| {
        let mut event = json!({"type":"committed","id":id,"agent":"main","kind":kind});
        event
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(event).unwrap(),
        ));
    };
    let call = |app: &mut App, id, name, call_id, args| {
        commit(
            app,
            id,
            "tool.call",
            json!({"entry":{"id":id,"agent":"main","turn":40,
            "type":"tool_call","tool":name,"tool_call_id":call_id,"args":args}}),
        );
    };
    let dispatch = |app: &mut App, id, call_id| {
        commit(
            app,
            id,
            "runtime.tool_dispatched",
            json!({"tool_call_id":call_id}),
        );
    };
    let result = |app: &mut App, id, call_id| {
        commit(
            app,
            id,
            "tool.result",
            json!({"entry":{"id":id,"agent":"main","turn":40,
            "type":"tool_result","tool_call_id":call_id,"status":"ok"}}),
        );
    };

    let first = heading(&app, &mut clock, 0, 80);
    assert!(
        first.contains("Waiting for model") && !first.contains("Waiting for job"),
        "{first}"
    );
    live(&mut app, json!({"type":"thinking"}), 53);
    assert!(heading(&app, &mut clock, 1, 80).contains("Thinking"));
    live(
        &mut app,
        json!({"type":"text","content":"Starting now."}),
        53,
    );
    assert!(heading(&app, &mut clock, 2, 80).contains("Replying"));
    live(
        &mut app,
        json!({"type":"tool_arguments","preparing_file_contents":true}),
        53,
    );
    let prepared = heading(&app, &mut clock, 3, 80);
    assert!(
        prepared.contains("Preparing file contents") && !prepared.contains("Writing file"),
        "{prepared}"
    );
    commit(&mut app, 54, "model.response", json!({}));
    call(
        &mut app,
        55,
        "write_file",
        "write-1",
        json!({"path":"notes.md","content":"private draft"}),
    );
    // Merely committing model arguments is not dispatch.
    assert!(!heading(&app, &mut clock, 4, 80).contains("Writing file"));
    dispatch(&mut app, 56, "write-1");
    assert!(heading(&app, &mut clock, 5, 80).contains("Using tools"));
    let writing = heading(&app, &mut clock, 300, 80);
    assert!(
        writing.contains("Writing file")
            && writing.contains("notes.md")
            && !writing.contains("private draft"),
        "{writing}"
    );
    result(&mut app, 57, "write-1");
    let cleared = heading(&app, &mut clock, 301, 80);
    assert!(!cleared.contains("notes.md"), "{cleared}");
    call(
        &mut app,
        58,
        "run_command",
        "command-1",
        json!({"program":"echo","args":["hello"]}),
    );
    dispatch(&mut app, 59, "command-1");
    assert!(heading(&app, &mut clock, 302, 80).contains("Using tools"));
    assert!(heading(&app, &mut clock, 600, 80).contains("Running command"));
    result(&mut app, 60, "command-1");

    // One pending wait is explicitly for job-23, not the most recent
    // registered job nor the count of independent background jobs.
    call(
        &mut app,
        61,
        "wait_job",
        "wait-1",
        json!({"job_id":"job-23"}),
    );
    dispatch(&mut app, 62, "wait-1");
    for (ms, width) in [(601, 19), (602, 80)] {
        let waiting = heading(&app, &mut clock, ms, width);
        assert!(waiting.contains("job-23"), "{waiting}");
    }
    result(&mut app, 63, "wait-1");
    let settled = heading(&app, &mut clock, 603, 80);
    assert!(
        !settled.contains("job-23") && !settled.contains("Waiting for job"),
        "{settled}"
    );
    commit(&mut app, 64, "runtime.step_prepared", json!({}));
    assert!(heading(&app, &mut clock, 604, 80).contains("Waiting for model"));
    live(&mut app, json!({"type":"tool_arguments"}), 64);
    assert!(heading(&app, &mut clock, 605, 80).contains("Preparing tool call"));

    commit(
        &mut app,
        65,
        "runtime.operation",
        json!({"operation":"permission_waiting","state_changed":true,"entry":{"id":65,"agent":"main","turn":40,"type":"permission","request":{"id":"approve-write"}}}),
    );
    assert!(heading(&app, &mut clock, 606, 80).contains("Waiting for approval"));
    commit(
        &mut app,
        66,
        "runtime.operation",
        json!({"operation":"permission_replied","state_changed":true,"entry":{"id":66,"agent":"main","turn":40,"type":"permission_reply","request_id":"approve-write","allow":true}}),
    );
    call(
        &mut app,
        67,
        "wait_job",
        "wait-2",
        json!({"job_id":"job-51"}),
    );
    dispatch(&mut app, 68, "wait-2");
    assert!(heading(&app, &mut clock, 607, 80).contains("job-51"));
    app.select(
        Address::new(app.active.session.clone(), "reviewer".into()),
        false,
        "Review".into(),
    );
    let switched = heading(&app, &mut clock, 608, 80);
    assert!(
        !switched.contains("job-51") && !switched.contains("Waiting for job"),
        "{switched}"
    );
}

#[test]
fn terminal_heading_reports_confirmed_failure_and_an_available_next_step() {
    use super::super::activity::{ActivityFrame, render_heading};
    use kuku_client::HistoryEntry;

    for (status, error, entry, expected) in [
        ("stopped", None, None, "Turn stopped"),
        ("interrupted", None, None, "Interrupted"),
        (
            "interrupted",
            Some("provider said no"),
            Some(json!({
                "id": 51, "agent":"main", "turn":40, "type":"error",
                "code":"http_status", "text":"provider said no"
            })),
            "Model request failed",
        ),
    ] {
        let mut app = app();
        app.select(app.active.clone(), true, "Settled".into());
        let selection = app.selection();
        let mut state = snapshot(&app, status, json!(null), json!(null));
        state.agent.turn = Some(40);
        state.agent.execution_id = None;
        state.agent.last_error = error.map(str::to_owned);
        app.update(Update::Loaded(
            selection.clone(),
            state,
            HistoryPage {
                entries: entry
                    .map(|value| serde_json::from_value::<HistoryEntry>(value).unwrap())
                    .into_iter()
                    .collect(),
                rewind_groups: vec![],
                next_before: None,
            },
        ));
        app.update(Update::WatchConnected(selection));
        for width in [32, 100] {
            let frame = ActivityFrame::test_frame(&app, None);
            let line = render_heading(&app, width, frame, false, false).0;
            assert!(line.width() <= width);
            let line = line.to_string();
            let action = if status == "stopped" {
                "Send"
            } else {
                "F2 Continue"
            };
            assert!(line.contains(action), "{line}");
            if width == 100 {
                assert!(line.contains(expected), "{line}");
            }
        }
    }
}

#[test]
fn stopped_queue_guidance_uses_the_existing_receipt_and_preserves_continue_on_narrow_width() {
    use kuku_client::{Input, QueuedInput};

    let mut app = app();
    app.select(app.active.clone(), true, "Paused queue".into());
    let mut state = snapshot(&app, "stopped", json!(null), json!(null));
    state.agent.execution_id = None;
    state.agent.pending_inputs = 1;
    state.pending.push(QueuedInput {
        input: Input {
            source: InputSource::Host("http".into()),
            ..Input::new("original-receipt", app.active.agent.clone(), "Follow-up")
        },
        receipt: Receipt {
            command_id: "original-receipt".into(),
            event_id: 50,
            agent: "main".into(),
        },
    });
    app.update(Update::Loaded(
        app.selection(),
        state,
        HistoryPage {
            entries: vec![],
            rewind_groups: vec![],
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(app.selection()));
    let wide = screen(&mut app, 90, 24);
    assert!(
        wide.contains("Turn stopped")
            && wide.contains("1 saved")
            && wide.contains("Continue session"),
        "{wide}"
    );
    assert!(!wide.contains("1 queued"), "{wide}");
    let narrow = screen(&mut app, 34, 20);
    assert!(
        narrow.contains("1 saved") && narrow.contains("Continue session"),
        "{narrow}"
    );
    assert!(!narrow.contains("Send message"));
    assert!(app.queue_input_available("original-receipt"));
}
