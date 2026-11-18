use std::path::PathBuf;

use kuku_client::{
    Acknowledgement, Error, HistoryPage, InputStatus, Receipt, Snapshot, Timing, UsageTotals,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::{json, Value};
use unicode_width::UnicodeWidthStr;

use super::{
    compact_token_count, render_segments, Field, Priority, Segment, StatusLine, SEPARATOR,
};
use crate::app::{Address, App};
use crate::config::StatusLineSettings;
use crate::jobs::{Job, Update};
use crate::theme::{ERROR, MODEL, WARNING};
use crate::ui;

fn connected_app() -> App {
    connected_workspace(PathBuf::from("/workspace"))
}

fn connected_workspace(workspace: PathBuf) -> App {
    let mut app = App::new(workspace.clone(), None);
    app.update(Update::Connected {
        workspace,
        endpoint: "http://127.0.0.1:6789".into(),
    });
    app
}

fn load_agent(app: &mut App, status: &str, queued: usize, question: Value, usage: UsageTotals) {
    app.select(app.active.clone(), true, "Example".into());
    let selection = app.selection();
    let snapshot: Snapshot = serde_json::from_value(json!({
        "session_id": app.active.session,
        "workspace": app.workspace,
        "cursor": 50,
        "pending": [],
        "agents": [],
        "agent": {
            "address": app.active.agent, "config": {}, "status": status,
            "pending_inputs": queued, "execution_id": 49, "question": question,
            "usage": usage,
        },
    }))
    .unwrap();
    app.update(Update::Loaded(
        selection.clone(),
        Box::new(snapshot),
        HistoryPage {
            entries: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));
}

fn measured_usage() -> UsageTotals {
    serde_json::from_value(json!({
        "input_tokens": 129_000_000,
        "output_tokens": 210_000,
        "cache_read_input_tokens": 96_750_000,
        "model_requests": 3,
        "unreported_requests": 0,
        "input_reported_requests": 3,
        "output_reported_requests": 3,
        "cache_measured_input_tokens": 129_000_000,
        "cache_measured_read_tokens": 96_750_000,
        "measured_output_tokens": 920,
        "measured_elapsed_us": 50_000_000,
        "latest_request": {
            "input_tokens": 89_000,
            "output_tokens": 400,
            "context_window_tokens": 100_000,
            "elapsed_us": 25_000_000,
        },
    }))
    .unwrap()
}

fn submit_draft(app: &mut App) -> String {
    app.send(Timing::NextTurn);
    let command = app
        .effects
        .iter()
        .rev()
        .find_map(|effect| match effect {
            Job::Save(command) => Some(command.clone()),
            _ => None,
        })
        .unwrap();
    let id = command.command_id.clone();
    app.update(Update::Saved(command, Ok(())));
    id
}

fn screen(app: &mut App, statusline: &StatusLine, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let settings = std::mem::replace(
        &mut app.statusline_settings,
        StatusLineSettings::in_memory(statusline.clone()),
    );
    terminal.draw(|frame| ui::render(frame, app)).unwrap();
    app.statusline_settings = settings;
    terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

#[test]
fn status_line_setting_orders_omits_and_hides_fields() {
    assert_eq!(
        StatusLine {
            fields: Some(vec![Field::Model, Field::Workspace, Field::Execution]),
        },
        StatusLine::parse(" model, workspace,execution ").unwrap()
    );
    assert_eq!(StatusLine::default(), StatusLine::parse("").unwrap());
    assert_eq!(
        StatusLine {
            fields: Some(Vec::new())
        },
        StatusLine::parse("none").unwrap()
    );
}

#[test]
fn status_line_setting_reports_duplicates_and_unknown_fields() {
    let duplicate = StatusLine::parse("workspace,workspace").unwrap_err();
    assert!(duplicate.contains("repeats workspace"));
    let unknown = StatusLine::parse("model,network").unwrap_err();
    assert!(unknown.contains("unknown field \"network\""));
}

#[test]
fn short_status_line_fits_complete_graphemes_and_keeps_urgent_segments() {
    let line = render_segments(
        vec![
            Segment::workspace("/dir/开发👩‍💻/最后".into()),
            Segment::labels(vec!["Waiting".into()], WARNING, Priority::Attention),
            Segment::labels(vec!["Offline".into()], ERROR, Priority::ConnectionProblem),
        ],
        17,
        SEPARATOR,
    )
    .unwrap();
    assert_eq!(
        vec!["Waiting", SEPARATOR, "Offline"],
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
    );
    let line = render_segments(
        vec![Segment::workspace(
            "/dir/e\u{0301}e\u{0301}e\u{0301}/👩‍💻末".into(),
        )],
        10,
        SEPARATOR,
    )
    .unwrap();
    assert!(line.width() <= 10);
    assert!(line.spans[0].content.ends_with("/👩‍💻末"));
}

#[test]
fn question_stays_visible_when_execution_is_hidden_or_the_service_is_offline() {
    let mut app = connected_app();
    load_agent(
        &mut app,
        "waiting",
        0,
        json!({
            "id": "q-1", "agent": "main", "turn": 1, "tool_call_id": "tool-1",
            "text": "Which test target?",
        }),
        UsageTotals::default(),
    );
    let custom = StatusLine::parse("workspace").unwrap();
    let rows = screen(&mut app, &custom, 66, 14);
    assert!(rows.iter().any(|row| row.contains("F2 Reply")));
    assert!(rows.iter().any(|row| row.contains("Which test target?")));
    assert_eq!("/workspace", rows.last().unwrap().trim());

    app.reconnect();
    app.update(Update::ConnectFailed(Error::Options(
        "fixture offline".into(),
    )));
    let rows = screen(&mut app, &StatusLine::default(), 29, 10);
    assert!(rows.last().unwrap().contains("Offline"));
    assert!(rows[..9].iter().any(|row| row.contains("Offline")));
    assert!(rows.iter().any(|row| row.contains("Which test target?")));
}

#[test]
fn status_line_distinguishes_execution_unconfirmed_input_queue_and_draft() {
    let mut app = connected_app();
    load_agent(&mut app, "running", 2, Value::Null, UsageTotals::default());
    app.pane_mut().draft.insert("First input");
    submit_draft(&mut app);
    app.pane_mut().draft.insert("Next unsent draft");
    let statusline = StatusLine::parse("execution,input").unwrap();
    let rows = screen(&mut app, &statusline, 70, 16);
    assert_eq!(
        "Running · Awaiting receipt, 2 queued, Draft",
        rows.last().unwrap().trim()
    );
}

#[test]
fn draft_receipt_and_copy_feedback_keep_the_footer_in_place() {
    let mut app = connected_app();
    load_agent(&mut app, "idle", 0, Value::Null, measured_usage());
    let statusline = StatusLine::default();
    let footer = screen(&mut app, &statusline, 140, 24).pop().unwrap();
    app.pane_mut().draft.insert("First input");
    let rows = screen(&mut app, &statusline, 140, 24);
    assert_eq!(footer, rows[23]);
    assert!(rows[19].trim_end().ends_with("Draft"));
    let id = submit_draft(&mut app);
    let rows = screen(&mut app, &statusline, 140, 24);
    assert_eq!(footer, rows[23]);
    assert!(rows[19].contains("Awaiting receipt"));
    app.update(Update::Sent(
        id.clone(),
        Ok(Acknowledgement {
            receipt: Receipt {
                command_id: id,
                event_id: 51,
                agent: "main".into(),
            },
            replayed: false,
            status: Some(InputStatus::Accepted),
        }),
    ));
    let rows = screen(&mut app, &statusline, 140, 24);
    assert_eq!(footer, rows[23]);
    assert!(rows[19].contains("Input accepted"));
    app.confirmation("Copied to clipboard");
    let rows = screen(&mut app, &statusline, 140, 24);
    assert_eq!(footer, rows[23]);
    assert!(rows[19].contains("Copied to clipboard"));
    assert_eq!(
        1,
        rows.iter()
            .filter(|row| row.contains("Copied to clipboard"))
            .count()
    );
    app.pane_mut().draft.insert("Second input");
    let rows = screen(&mut app, &statusline, 140, 24);
    assert_eq!(footer, rows[23]);
    assert!(rows[19].contains("Draft") && !rows[19].contains("Input accepted"));
    assert!(!rows.iter().any(|row| row.contains("Copied to clipboard")));
}

#[test]
fn composer_follows_wrapped_draft_rows_and_caps_the_visible_paste() {
    let mut app = connected_app();
    let statusline = StatusLine::default();
    let rows = screen(&mut app, &statusline, 80, 20);
    assert!(rows[15].contains("To main"));
    assert!(rows[17].contains("Type a prompt"));
    app.pane_mut().draft.insert(&"a".repeat(76));
    let rows = screen(&mut app, &statusline, 80, 20);
    assert!(rows[14].contains("To main") && rows[17].trim_end().ends_with('a'));
    app.pane_mut()
        .draft
        .set_text("line 1\nline 2\nline 3\nline 4\nline 5\n中文\nline 7".into());
    let rows = screen(&mut app, &statusline, 80, 20);
    assert!(rows[12].contains("To main"));
    assert!(rows[14].contains("line 4"));
    assert!(rows[16].contains("中") && rows[16].contains("文"));
    assert!(rows[17].contains("line 7"));
    let rows = screen(&mut app, &statusline, 29, 6);
    assert!(rows[2].contains("To main"));
    assert!(rows[2].contains("Ready") && rows[4].contains("line 7"));
}

#[test]
fn compact_token_counts_use_decimal_units_with_integer_arithmetic() {
    assert_eq!("12.4K", compact_token_count(12_432));
    assert_eq!("1.25M", compact_token_count(1_250_000));
    assert_eq!("999K", compact_token_count(999_999));
    assert_eq!("1B", compact_token_count(1_000_000_000));
    assert_eq!("18446744073B", compact_token_count(u64::MAX));
}

#[test]
fn default_status_line_shows_context_tps_and_workspace_in_one_row() {
    let workspace = PathBuf::from("/workspace")
        .join("archived-projects".repeat(12))
        .join("work-right");
    let mut app = connected_workspace(workspace.clone());
    load_agent(&mut app, "running", 2, Value::Null, measured_usage());
    app.pane_mut().draft.insert("Local draft");
    let statusline = StatusLine::default();
    let rows = screen(&mut app, &statusline, 140, 24);
    let footer = rows[23].trim();
    for label in [
        "Connected",
        "Agent main",
        "work-right",
        "Context 89% used",
        "18.4 TPS",
    ] {
        assert!(footer.contains(label), "missing {label:?} in {footer}");
    }
    assert!(footer.starts_with("Model pending  Connected  Agent main"));
    for detail in ["Running", "http://", "129M in", "210K out", "Cache hit"] {
        assert!(!footer.contains(detail), "{footer}");
    }
    assert!(!footer.contains(workspace.to_string_lossy().as_ref()));
    assert!(rows[19].contains("To main") && rows[19].contains("2 queued, Draft"));
    assert!(rows[19].contains("Running"));
    assert!(rows[21].contains("Local draft"));
    let rows = screen(&mut app, &statusline, 80, 24);
    assert!(rows[23].contains("Ctx 89%"));
    assert!(rows[23].contains("work-right") && rows[21].contains("Local draft"));
    let rows = screen(&mut app, &statusline, 29, 24);
    assert!(rows[20].contains("Running") && rows[20].contains("To main"));
    let rows = screen(&mut app, &StatusLine::parse("none").unwrap(), 80, 24);
    assert!(
        rows[23].contains("Local draft"),
        "the status row should be reclaimed"
    );
}

#[test]
fn wide_status_line_keeps_metrics_anchored_while_identity_width_changes() {
    let layout = StatusLine::default();
    let mut short = connected_workspace(PathBuf::from("/short"));
    let mut long = connected_workspace(PathBuf::from("/really-long-directory".repeat(20)));
    for app in [&mut short, &mut long] {
        load_agent(app, "running", 0, Value::Null, measured_usage());
    }
    let first = screen(&mut short, &layout, 120, 24);
    long.pane_mut().draft.insert("正在输入");
    let second = screen(&mut long, &layout, 120, 24);
    let measures = "Context 89% used  18.4 TPS";
    assert_ne!(first[23], second[23]);
    let position = |row: &str| row.split_once(measures).map(|(prefix, _)| prefix.width());
    assert_eq!(position(&first[23]), position(&second[23]));
    assert!(first[23].trim_end().ends_with(measures));
    assert!(second[23].trim_end().ends_with(measures));
    assert_eq!("", second[22].trim());
}

#[test]
fn default_model_field_keeps_the_resolved_identifier_first_at_every_width() {
    let mut app = connected_app();
    let version = app
        .effects
        .iter()
        .find_map(|effect| match effect {
            Job::Model(_, version) => Some(*version),
            _ => None,
        })
        .expect("initial model request");
    let model = serde_json::from_value(json!({
        "connection": "configured-model-gateway",
        "protocol": "openai-responses",
        "model": "openai/gpt-6-luna",
        "context_window_tokens": 100_000,
        "max_output_tokens": 4_000,
        "settings": {"reasoning_effort": "high"},
        "capabilities": {"profile": "openai", "reasoning_efforts": ["high"], "service_tiers": []},
    }))
    .unwrap();
    app.update(Update::DefaultModel(version, Ok(model)));
    let layout = StatusLine::default();
    for width in [60, 80, 140] {
        let rows = screen(&mut app, &layout, width, 20);
        assert!(
            rows[19].trim().starts_with("openai/gpt-6-luna "),
            "{}",
            rows[19]
        );
    }
    let configured = StatusLine::parse("model,connection").unwrap();
    let rows = screen(&mut app, &configured, 140, 20);
    assert!(rows[19]
        .trim()
        .starts_with("openai/gpt-6-luna (configured-model-gateway · high) ·"));
    let line = layout.render_line(&app, 80, app.pane().activity()).unwrap();
    assert_eq!(Some(MODEL), line.spans[0].style.fg);
}

#[test]
fn configured_status_line_can_show_full_metrics_and_connection_details() {
    let mut app = connected_app();
    load_agent(&mut app, "running", 0, Value::Null, measured_usage());
    let statusline = StatusLine::parse(
        "connection,execution,agent,workspace,model,context,tokens_in,tokens_out,cache,tps",
    )
    .unwrap();
    let rows = screen(&mut app, &statusline, 140, 24);
    let footer = rows[23].trim();
    assert!(footer.starts_with("Connected Local · Running"), "{footer}");
    assert!(
        footer.ends_with("Context 89% used · 129M in · 210K out · Cache hit 75% · 18.4 TPS"),
        "{footer}"
    );
    assert!(rows[21].contains("Type a prompt"));
}

#[test]
fn selected_agent_metrics_change_with_the_address_and_custom_order() {
    let mut app = connected_app();
    load_agent(&mut app, "running", 0, Value::Null, measured_usage());
    let main = app.active.clone();
    let statusline =
        StatusLine::parse("agent,tps,context,tokens_out,cache,tokens_in,workspace").unwrap();
    let rows = screen(&mut app, &statusline, 54, 20);
    assert_eq!(
        "Agent main · 18.4 TPS · 210K out · /workspace",
        rows[19].trim()
    );
    assert!(rows[17].contains("Type a prompt"));

    app.select(
        Address::new(main.session.clone(), "reviewer".into()),
        true,
        "Example".into(),
    );
    let rows = screen(&mut app, &statusline, 120, 20);
    assert_eq!(
        "Agent reviewer · — TPS · Context — · — out · Cache hit — · — in · /workspace",
        rows[19].trim()
    );
    let reviewer: UsageTotals = serde_json::from_value(json!({
        "model_requests": 2,
        "unreported_requests": 1,
        "input_tokens": 1_204,
        "output_tokens": 0,
        "input_reported_requests": 1,
        "output_reported_requests": 2,
    }))
    .unwrap();
    load_agent(&mut app, "idle", 0, Value::Null, reviewer);
    let rows = screen(&mut app, &statusline, 120, 20);
    assert_eq!(
        "Agent reviewer · — TPS · Context — · 0 out · Cache hit — · ≥1.2K in · /workspace",
        rows[19].trim()
    );
    app.select(main, true, "Example".into());
    let rows = screen(&mut app, &statusline, 120, 20);
    assert_eq!(
        "Agent main · 18.4 TPS · Context 89% used · 210K out · Cache hit 75% · 129M in · /workspace",
        rows[19].trim()
    );
}

#[test]
fn next_step_hides_stale_context_and_retains_average_throughput() {
    let mut app = connected_app();
    load_agent(&mut app, "running", 0, Value::Null, measured_usage());
    let statusline = StatusLine::parse("context,tokens_in,tokens_out,cache,tps").unwrap();
    let step = serde_json::from_value(json!({
        "type": "committed", "id": 51, "agent": "main", "kind": "runtime.step_prepared"
    }))
    .unwrap();
    app.update(Update::Observation(app.selection(), step));
    let rows = screen(&mut app, &statusline, 80, 20);
    assert_eq!(
        "Context — · 129M in · 210K out · Cache hit 75% · 18.4 TPS",
        rows[19].trim()
    );
}

#[test]
fn known_zero_measurements_have_distinct_labels_from_missing_metrics() {
    let mut app = connected_app();
    let statusline = StatusLine::parse("context,tokens_out,cache,tps").unwrap();
    let rows = screen(&mut app, &statusline, 80, 20);
    assert_eq!("Context — · — out · Cache hit — · — TPS", rows[19].trim());
    let usage: UsageTotals = serde_json::from_value(json!({
        "input_tokens": 100,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "model_requests": 2,
        "unreported_requests": 0,
        "input_reported_requests": 2,
        "output_reported_requests": 2,
        "cache_measured_input_tokens": 100,
        "cache_measured_read_tokens": 0,
        "measured_output_tokens": 0,
        "measured_elapsed_us": 1_000_000,
        "latest_request": {
            "input_tokens": 0,
            "output_tokens": 0,
            "context_window_tokens": 100_000,
            "elapsed_us": 1_000_000,
        },
    }))
    .unwrap();
    load_agent(&mut app, "idle", 0, Value::Null, usage);
    let rows = screen(&mut app, &statusline, 80, 20);
    assert_eq!(
        "Context 0% used · 0 out · Cache hit 0% · 0 TPS",
        rows[19].trim()
    );
}

#[test]
fn cache_hit_stays_visible_after_an_attempt_without_provider_usage() {
    let mut app = connected_app();
    let usage = UsageTotals {
        input_tokens: Some(1_098_058),
        cache_read_input_tokens: Some(1_065_472),
        cache_measured_input_tokens: 1_098_058,
        cache_measured_read_tokens: 1_065_472,
        model_requests: 44,
        unreported_requests: 1,
        ..UsageTotals::default()
    };
    load_agent(&mut app, "running", 0, Value::Null, usage);
    let statusline = StatusLine::parse("cache").unwrap();
    let rows = screen(&mut app, &statusline, 58, 20);
    assert_eq!("Cache hit 97%", rows[19].trim());
}
