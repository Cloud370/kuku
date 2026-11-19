use std::path::{Path, PathBuf};

use kuku_client::{
    Acknowledgement, Error, HistoryPage, Observation, Receipt, RetryFailureKind, RetryPhase,
    RetrySource, RetryStopReason, Snapshot, Timing,
};
use serde_json::{json, Value};

use super::{Activity, App, HistoryEntry, InputStatus, ServiceStatus, SubmissionStatus};
use crate::app::Address;
use crate::jobs::{Job, Update};
use crate::transcript::HistoryRequest;

fn connected() -> App {
    let workspace = PathBuf::from("/workspace");
    let mut app = App::new(workspace.clone(), None);
    assert_eq!(ServiceStatus::Connecting, app.service_status());
    app.update(Update::Connected {
        endpoint: "http://127.0.0.1:6789".into(),
        workspace,
    });
    assert_eq!(ServiceStatus::Connected, app.service_status());
    app.effects.clear();
    app
}

fn snapshot(
    app: &App,
    cursor: u64,
    status: &str,
    execution: Option<u64>,
    queued: usize,
    progress: Value,
    error: Option<&str>,
) -> Box<Snapshot> {
    Box::new(
        serde_json::from_value(json!({
            "session_id": app.active.session,
            "workspace": app.workspace,
            "cursor": cursor,
            "agents": [],
            "pending": [],
            "progress": progress,
            "agent": {
                "address": app.active.agent,
                "status": status,
                "turn": 5,
                "pending_inputs": queued,
                "execution_id": execution,
                "last_error": error,
                "config": {},
                "usage": {"model_requests": 0, "unreported_requests": 0},
            }
        }))
        .unwrap(),
    )
}

fn loaded(app: &mut App, snapshot: Box<Snapshot>, entries: Vec<HistoryEntry>) {
    let selection = app.selection();
    app.update(Update::Loaded(
        selection.clone(),
        snapshot,
        HistoryPage {
            entries,
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(selection));
}

fn observe(app: &mut App, frame: Value) {
    app.update(Update::Observation(
        app.selection(),
        serde_json::from_value(frame).unwrap(),
    ));
}

#[test]
fn remote_message_rewind_invalidates_history_and_requests_recent_page() {
    let mut app = connected();
    let selection = app.selection();
    observe(
        &mut app,
        json!({
            "type":"committed",
            "id":1,
            "agent":"main",
            "kind":"runtime.operation",
            "entry":null,
            "state_changed":false,
            "turn":null,
            "turn_started_at_ms":null,
            "request_id":null,
            "tool_call_id":null,
            "operation":"message_rewind",
            "delivery":null,
            "settlement":null,
            "cleanup_pending":null,
        }),
    );
    assert_eq!(Some(HistoryRequest::Recent), app.pane().history_in_flight);
    assert!(app.effects.iter().any(|job| matches!(
        job,
        Job::History(target, HistoryRequest::Recent) if *target == selection
    )));
}

#[test]
fn remote_message_rewind_discards_an_inflight_older_page() {
    let mut app = connected();
    let selection = app.selection();
    let stale_request = HistoryRequest::Older(10);
    app.pane_mut().history_in_flight = Some(stale_request);
    observe(
        &mut app,
        json!({
            "type":"committed",
            "id":1,
            "agent":"main",
            "kind":"runtime.operation",
            "entry":null,
            "state_changed":false,
            "turn":null,
            "turn_started_at_ms":null,
            "request_id":null,
            "tool_call_id":null,
            "operation":"message_rewind",
            "delivery":null,
            "settlement":null,
            "cleanup_pending":null,
        }),
    );
    assert!(app.effects.is_empty());

    app.update(Update::History(
        selection.clone(),
        stale_request,
        Ok(HistoryPage {
            entries: Vec::new(),
            next_before: Some(5),
        }),
    ));

    assert_eq!(Some(HistoryRequest::Recent), app.pane().history_in_flight);
    assert!(app.effects.iter().any(|job| matches!(
        job,
        Job::History(target, HistoryRequest::Recent) if *target == selection
    )));
}

fn streaming(app: &mut App, cursor: u64) {
    observe(
        app,
        json!({
            "type":"live", "agent":"main", "execution_id":5, "cursor":cursor,
            "event":{"type":"text","content":"First tokens"},
        }),
    );
}

#[test]
fn only_observed_reasoning_sets_thinking_and_visible_text_sets_streaming() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let attached = snapshot(
        &app,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"waiting_for_model"}),
        None,
    );
    loaded(&mut app, attached, Vec::new());
    let think = |app: &mut App, cursor, event| {
        observe(
            app,
            json!({
                "type":"live", "agent":"main", "execution_id":5, "cursor":cursor,
                "event":event,
            }),
        );
    };
    think(&mut app, 7, json!({"type":"thinking", "content":""}));
    assert_eq!(Activity::WaitingForModel, app.pane().activity());
    think(&mut app, 7, json!({"type":"thinking"}));
    assert_eq!(Activity::Thinking, app.pane().activity());
    streaming(&mut app, 8);
    assert_eq!(Activity::Streaming, app.pane().activity());
    think(
        &mut app,
        9,
        json!({"type":"thinking", "content":"legacy reasoning marker"}),
    );
    assert_eq!(Activity::Thinking, app.pane().activity());
    think(&mut app, 9, json!({"type":"tool_arguments"}));
    assert_eq!(Activity::ToolArguments, app.pane().activity());
    let screen = app.pane_mut().transcript.project(80, 12);
    assert!(screen
        .lines
        .iter()
        .any(|line| line.to_string().contains("First tokens")));
    assert!(!screen
        .lines
        .iter()
        .any(|line| line.to_string().contains("legacy reasoning marker")));
    committed(&mut app, 10, "runtime.model_retry", json!({}));
    assert_eq!(Activity::RetryingModel, app.pane().activity());
}

#[test]
fn model_recovery_transitions_after_output_and_retires_the_old_preview() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let attached = snapshot(
        &app,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"waiting_for_model"}),
        None,
    );
    loaded(&mut app, attached, Vec::new());
    streaming(&mut app, 7);

    observe(
        &mut app,
        json!({
            "type":"live","agent":"main","execution_id":5,"cursor":8,
            "event":{"type":"model_recovery","attempt":2,"max_attempts":4,
                "reason":"length","from_request_id":"failed","to_request_id":"recovered"}
        }),
    );

    assert_eq!(Activity::RetryingModel, app.pane().activity());
    assert_eq!(
        Some(RetrySource::Recovery),
        app.pane().retry_metadata().map(|retry| retry.source)
    );
    let transcript = app
        .pane_mut()
        .transcript
        .project(80, 12)
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!transcript.contains("First tokens"));
}

#[test]
fn retry_snapshot_and_replay_keep_typed_progress_until_output_or_terminal_failure() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let retry = json!({
        "source":"runtime.model_retry","phase":"waiting","attempt":2,
        "failed_attempt":1,"max_attempts":4,"failed_kind":"rate_limited",
        "request_id":"request-2","delay_ms":5000,"ts":"2025-01-01T00:00:00Z",
        "will_retry":true
    });
    let attached = snapshot(
        &app,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"retrying_model","retry":retry}),
        None,
    );
    loaded(&mut app, attached, Vec::new());
    assert_eq!(Activity::RetryingModel, app.pane().activity());
    assert_eq!(
        Some(RetryPhase::Waiting),
        app.pane().retry_metadata().map(|item| item.phase)
    );

    let retry_entry = json!({
        "id":7,"agent":"main","turn":5,"type":"retry","text":null,
        "retry":{
            "source":"runtime.model_retry","phase":"started","attempt":2,
            "failed_kind":"rate_limited","request_id":"request-2","will_retry":true
        }
    });
    committed(
        &mut app,
        7,
        "runtime.model_retry",
        json!({"entry":retry_entry}),
    );
    committed(
        &mut app,
        7,
        "runtime.model_retry",
        json!({"entry":retry_entry}),
    );
    assert_eq!(
        Some(2),
        app.pane().retry_metadata().and_then(|item| item.attempt)
    );
    assert_eq!(
        Some(RetryFailureKind::RateLimited),
        app.pane()
            .retry_metadata()
            .and_then(|item| item.failed_kind)
    );

    streaming(&mut app, 8);
    assert_eq!(Activity::Streaming, app.pane().activity());
    assert!(app.pane().retry_metadata().is_none());

    committed(
        &mut app,
        9,
        "runtime.model_retry",
        json!({"entry":{
            "id":9,"agent":"main","turn":5,"type":"retry","text":null,
            "retry":{
                "source":"runtime.model_retry","phase":"stopped","attempt":2,
                "failed_kind":"transport","will_retry":false,
                "stop_reason":"stream_started"
            }
        }}),
    );
    streaming(&mut app, 10);
    assert_eq!(
        Some(RetryStopReason::StreamStarted),
        app.pane()
            .retry_metadata()
            .and_then(|item| item.stop_reason)
    );
    committed(
        &mut app,
        11,
        "model.error",
        json!({"entry":{
            "id":11,"agent":"main","turn":5,"type":"error","code":"transport",
            "text":"connection failed"
        }}),
    );
    assert_ne!(Activity::RetryingModel, app.pane().activity());
    assert_eq!(
        Some(RetryStopReason::StreamStarted),
        app.pane()
            .retry_metadata()
            .and_then(|item| item.stop_reason)
    );
    assert!(app
        .pane_mut()
        .transcript
        .project(80, 10)
        .lines
        .iter()
        .any(|line| line.to_string().contains("First tokens")));
    committed(
        &mut app,
        12,
        "turn.interrupted",
        json!({"entry":{
            "id":12,"agent":"main","turn":5,"type":"status",
            "status":"interrupted","text":"transport"
        }}),
    );
    assert_eq!(
        Some(RetryStopReason::StreamStarted),
        app.pane()
            .retry_metadata()
            .and_then(|item| item.stop_reason)
    );
    assert!(app
        .pane_mut()
        .transcript
        .project(80, 10)
        .lines
        .iter()
        .any(|line| line.to_string().contains("First tokens")));
    committed(
        &mut app,
        13,
        "runtime.operation",
        json!({"state_changed":true,"operation":"turn_settled"}),
    );
    assert_eq!(
        Some(RetryStopReason::StreamStarted),
        app.pane()
            .retry_metadata()
            .and_then(|item| item.stop_reason)
    );
    assert!(app
        .pane_mut()
        .transcript
        .project(80, 10)
        .lines
        .iter()
        .any(|line| line.to_string().contains("First tokens")));

    let stopped_retry = json!({
        "source":"runtime.model_retry","phase":"stopped","attempt":2,
        "failed_kind":"transport","will_retry":false,
        "stop_reason":"stream_started"
    });
    let selection = app.selection();
    let settled = snapshot(
        &app,
        13,
        "interrupted",
        None,
        0,
        json!({"phase":"retry_stopped","retry":stopped_retry}),
        Some("connection failed"),
    );
    app.update(Update::Refreshed(selection, Ok(settled)));
    assert_eq!(
        Some(RetryStopReason::StreamStarted),
        app.pane()
            .retry_metadata()
            .and_then(|item| item.stop_reason)
    );
    assert!(app
        .pane_mut()
        .transcript
        .project(80, 10)
        .lines
        .iter()
        .any(|line| line.to_string().contains("First tokens")));

    let mut reattached = connected();
    reattached.select(reattached.active.clone(), true, String::new());
    let attached_snapshot = snapshot(
        &reattached,
        13,
        "interrupted",
        None,
        0,
        json!({"phase":"retry_stopped","retry":stopped_retry}),
        Some("connection failed"),
    );
    loaded(
        &mut reattached,
        attached_snapshot,
        vec![
            serde_json::from_value(json!({
                "id":9,"agent":"main","turn":5,"type":"retry",
                "retry":stopped_retry
            }))
            .unwrap(),
            serde_json::from_value(json!({
                "id":11,"agent":"main","turn":5,"type":"error",
                "code":"transport","text":"connection failed"
            }))
            .unwrap(),
            serde_json::from_value(json!({
                "id":12,"agent":"main","turn":5,"type":"status",
                "status":"interrupted","text":"transport"
            }))
            .unwrap(),
        ],
    );
    assert_eq!(
        Some(RetryStopReason::StreamStarted),
        reattached
            .pane()
            .retry_metadata()
            .and_then(|item| item.stop_reason)
    );
    let history = reattached
        .pane_mut()
        .transcript
        .project(80, 10)
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!history.contains("First tokens"));
}

#[test]
fn explicit_agent_stop_clears_active_retry_progress_and_new_execution_resets_it() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let retry = json!({
        "source":"runtime.model_retry","phase":"waiting","attempt":2,
        "failed_kind":"rate_limited","will_retry":true
    });
    let attached = snapshot(
        &app,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"retrying_model","retry":retry}),
        None,
    );
    loaded(&mut app, attached, Vec::new());
    assert_eq!(
        Some(true),
        app.pane().retry_metadata().and_then(|item| item.will_retry)
    );

    committed(
        &mut app,
        7,
        "runtime.operation",
        json!({
            "state_changed":true,"operation":"agent_stopped",
            "entry":{
                "id":7,"agent":"main","turn":5,"type":"status",
                "status":"interrupted","execution_id":5,
                "text":"Interrupted partial reply","truncated":false
            }
        }),
    );
    assert!(app.pane().retry_metadata().is_none());
    let stopped = app
        .pane_mut()
        .transcript
        .project(80, 10)
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(1, stopped.matches("Interrupted partial reply").count());

    let mut next = connected();
    next.select(next.active.clone(), true, String::new());
    let attached = snapshot(
        &next,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"retrying_model","retry":retry}),
        None,
    );
    loaded(&mut next, attached, Vec::new());
    committed(
        &mut next,
        7,
        "runtime.operation",
        json!({"state_changed":true,"operation":"execution_started"}),
    );
    assert!(next.pane().retry_metadata().is_none());
}

#[test]
fn newer_output_and_cancellation_reject_stale_retry_frames() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let attached = snapshot(
        &app,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"waiting_for_model"}),
        None,
    );
    loaded(&mut app, attached, Vec::new());
    committed(
        &mut app,
        7,
        "model.response",
        json!({"entry":{"id":7,"agent":"main","turn":5,"type":"assistant","text":"done"}}),
    );
    committed(
        &mut app,
        7,
        "runtime.model_retry",
        json!({"entry":{
            "id":7,"agent":"main","turn":5,"type":"retry","text":null,
            "retry":{"source":"runtime.model_retry","attempt":2,"will_retry":true}
        }}),
    );
    assert!(app.pane().retry_metadata().is_none());
    committed(&mut app, 8, "turn.cancelled", json!({}));
    assert!(app.pane().retry_metadata().is_none());
}

#[test]
fn selected_reply_preview_survives_attachment_and_replaces_matching_deltas() {
    let mut app = connected();
    app.select(
        Address::new(app.active.session.clone(), "reviewer".into()),
        true,
        String::new(),
    );
    let mut current = snapshot(
        &app,
        8,
        "running",
        Some(5),
        0,
        json!({"phase":"thinking"}),
        None,
    );
    current.reply_preview = Some(
        serde_json::from_value(json!({
            "execution_id":5,"turn":5,"turn_number":1,"text":"First reply",
            "truncated":false,"phase":"thinking"
        }))
        .unwrap(),
    );
    loaded(&mut app, current, Vec::new());
    let text = |app: &mut App| {
        app.pane_mut()
            .transcript
            .project(80, 12)
            .lines
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(Activity::Thinking, app.pane().activity());
    assert_eq!(1, text(&mut app).matches("First reply").count());

    observe(
        &mut app,
        json!({
            "type":"live","agent":"reviewer","execution_id":5,"cursor":9,"turn":5,"turn_number":1,
            "event":{"type":"reply_preview","content":"First reply with more","truncated":false,
                "phase":"tool_arguments"}
        }),
    );
    assert_eq!(Activity::ToolArguments, app.pane().activity());
    observe(
        &mut app,
        json!({
            "type":"live","agent":"reviewer","execution_id":5,"cursor":9,"turn":5,"turn_number":1,
            "event":{"type":"text","content":" text"}
        }),
    );
    assert_eq!(Activity::Streaming, app.pane().activity());
    let body = text(&mut app);
    assert!(body.contains("First reply with more text"));
    assert_eq!(1, body.matches("First reply").count());

    observe(
        &mut app,
        json!({
            "type":"committed","agent":"reviewer","id":10,"kind":"model.response"
        }),
    );
    observe(
        &mut app,
        json!({
            "type":"committed","agent":"reviewer","id":11,"kind":"message.assistant",
            "entry":{"type":"assistant","id":11,"agent":"reviewer","turn":5,"turn_number":1,
                "text":"First reply with more text"}
        }),
    );
    observe(
        &mut app,
        json!({
            "type":"committed","agent":"reviewer","id":12,"kind":"runtime.step_prepared"
        }),
    );
    observe(
        &mut app,
        json!({
            "type":"live","agent":"reviewer","execution_id":5,"cursor":9,"turn":5,
            "event":{"type":"reply_preview","content":"Obsolete previous request","truncated":false,
                "phase":"streaming"}
        }),
    );
    assert_eq!(Activity::WaitingForModel, app.pane().activity());
    let settled = text(&mut app);
    assert_eq!(1, settled.matches("First reply with more text").count());
    assert!(!settled.contains("Obsolete"));
}

fn committed(app: &mut App, id: u64, kind: &str, details: Value) {
    let mut frame = json!({"type":"committed","id":id,"agent":"main","kind":kind});
    frame
        .as_object_mut()
        .unwrap()
        .extend(details.as_object().unwrap().clone());
    observe(app, frame);
}

fn tool_result(app: &mut App, id: u64, tool_call_id: &str) {
    committed(
        app,
        id,
        "tool.result",
        json!({"entry":{"id":id,"agent":"main","type":"tool_result","tool_call_id":tool_call_id}}),
    );
}

fn usage_snapshot(app: &App, cursor: u64, input: Option<u64>) -> Box<Snapshot> {
    let mut snapshot = snapshot(app, cursor, "running", Some(5), 0, Value::Null, None);
    snapshot.agent.usage.input_tokens = input;
    snapshot
}

fn input_tokens(app: &App) -> Option<u64> {
    app.pane().snapshot.as_ref()?.agent.usage.input_tokens
}

#[test]
fn footer_workspace_uses_the_resolved_service_path_and_active_agent_override() {
    let endpoint = "http://127.0.0.1:6789".to_owned();
    let mut app = App::new(PathBuf::from("/client-local"), Some(endpoint.clone()));
    assert_eq!(None, app.resolved_workspace());
    app.update(Update::Connected {
        endpoint,
        workspace: PathBuf::from("/host-workspace"),
    });
    assert_eq!(Some(Path::new("/host-workspace")), app.resolved_workspace());

    let main = app.active.clone();
    app.select(main.clone(), true, String::new());
    let mut active = usage_snapshot(&app, 6, None);
    active.agent.config.workspace = Some(PathBuf::from("/host-workspace/next"));
    active.agent.active_config = Some(kuku_client::AgentConfig {
        workspace: Some(PathBuf::from("/host-workspace/active")),
        ..kuku_client::AgentConfig::default()
    });
    loaded(&mut app, active, Vec::new());
    app.update(Update::WatchReconnecting(
        app.selection(),
        "disconnected".into(),
    ));
    assert_eq!(
        Some(Path::new("/host-workspace/active")),
        app.resolved_workspace()
    );
    app.select(
        Address::new(main.session, "reviewer".into()),
        true,
        String::new(),
    );
    assert_eq!(Some(Path::new("/host-workspace")), app.resolved_workspace());
}

#[test]
fn committed_responses_refresh_usage_during_the_turn_and_share_a_covering_snapshot() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let initial = usage_snapshot(&app, 6, None);
    loaded(&mut app, initial, Vec::new());
    app.effects.clear();
    committed(&mut app, 7, "model.response", json!({"agent": "reviewer"}));
    assert!(app.effects.is_empty());
    committed(&mut app, 8, "model.response", json!({}));
    assert!(matches!(app.effects.pop(), Some(Job::Refresh(_))));
    committed(&mut app, 9, "model.response", json!({}));
    assert!(app.effects.is_empty());

    let first = usage_snapshot(&app, 10, Some(410));
    app.update(Update::Refreshed(app.selection(), Ok(first)));
    assert_eq!(Some(410), input_tokens(&app));
    assert!(
        app.effects.is_empty(),
        "the returned cursor covers queued work"
    );
    committed(&mut app, 10, "model.response", json!({}));
    assert!(app.effects.is_empty());

    committed(&mut app, 11, "model.response", json!({}));
    assert!(matches!(app.effects.pop(), Some(Job::Refresh(_))));
    committed(&mut app, 12, "model.response", json!({}));
    let partial = usage_snapshot(&app, 11, Some(500));
    app.update(Update::Refreshed(app.selection(), Ok(partial)));
    assert!(matches!(app.effects.as_slice(), [Job::Refresh(_)]));
}

#[test]
fn new_step_or_configuration_hides_the_previous_request_sample() {
    let boundaries = [
        ("runtime.step_prepared", json!({})),
        (
            "runtime.operation",
            json!({"operation":"step_configured","state_changed":true}),
        ),
    ];
    for (kind, event) in boundaries {
        let mut app = connected();
        app.select(app.active.clone(), true, String::new());
        let mut measured = usage_snapshot(&app, 6, Some(50));
        measured.agent.usage.latest_request = Some(
            serde_json::from_value(json!({
                "input_tokens":50,"output_tokens":20,"context_window_tokens":100,"elapsed_us":20_000
            }))
            .unwrap(),
        );
        loaded(&mut app, measured, Vec::new());
        app.effects.clear();
        assert_eq!(
            Some(50.0),
            app.pane()
                .current_request_usage()
                .unwrap()
                .context_percent()
        );

        committed(&mut app, 7, kind, event);
        assert!(app.pane().current_request_usage().is_none());
        assert_eq!(Some(50), input_tokens(&app));
        assert!(matches!(app.effects.pop(), Some(Job::Refresh(_))));

        committed(&mut app, 8, "model.response", json!({}));
        let unreported = usage_snapshot(&app, 8, Some(50));
        app.update(Update::Refreshed(app.selection(), Ok(unreported)));
        let current = app.pane().current_request_usage().unwrap();
        assert_eq!(None, current.context_percent());
        assert_eq!(None, current.output_tokens_per_second());
        assert!(app.effects.is_empty());
    }
}

#[test]
fn usage_reloads_on_reconnect_and_old_updates_stay_with_their_target_generation() {
    let mut app = connected();
    let main = app.active.clone();
    app.select(main.clone(), true, String::new());
    let initial = usage_snapshot(&app, 7, Some(62));
    loaded(&mut app, initial, Vec::new());
    let original = app.selection();
    app.effects.clear();

    app.update(Update::WatchReconnecting(
        original.clone(),
        "disconnected".into(),
    ));
    assert_eq!(Some(62), input_tokens(&app));
    app.update(Update::WatchConnected(original.clone()));
    app.update(Update::Observation(
        original.clone(),
        Observation::Lagged { skipped: 0 },
    ));
    assert!(matches!(app.effects.as_slice(), [Job::RefreshProgress(_)]));
    let delayed = usage_snapshot(&app, 13, Some(120));
    app.select(
        Address::new(main.session.clone(), "reviewer".into()),
        true,
        String::new(),
    );
    let reviewer = usage_snapshot(&app, 9, None);
    loaded(&mut app, reviewer, Vec::new());
    app.update(Update::Refreshed(original.clone(), Ok(delayed.clone())));
    assert_eq!(None, input_tokens(&app));

    app.select(main, true, String::new());
    let attached = usage_snapshot(&app, 12, Some(108));
    loaded(&mut app, attached, Vec::new());
    app.update(Update::Refreshed(original, Ok(delayed)));
    assert_eq!(Some(108), input_tokens(&app));
}

#[test]
fn upstream_wait_stream_parallel_tools_and_settlement_keep_service_connected() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let attached = snapshot(
        &app,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"waiting_for_model"}),
        None,
    );
    loaded(&mut app, attached, Vec::new());
    assert_eq!(Activity::WaitingForModel, app.pane().activity());
    assert_eq!(ServiceStatus::Connected, app.service_status());
    streaming(&mut app, 7);
    assert_eq!(Activity::Streaming, app.pane().activity());
    committed(&mut app, 8, "model.response", json!({}));
    streaming(&mut app, 7);
    assert_eq!(Activity::Running, app.pane().activity());

    committed(
        &mut app,
        9,
        "runtime.tool_dispatched",
        json!({"tool_call_id": "tool-a"}),
    );
    committed(
        &mut app,
        10,
        "runtime.tool_dispatched",
        json!({"tool_call_id": "tool-b"}),
    );
    assert_eq!(Activity::RunningTools, app.pane().activity());
    tool_result(&mut app, 11, "tool-a");
    assert_eq!(Activity::RunningTools, app.pane().activity());
    tool_result(&mut app, 12, "tool-b");
    assert_eq!(Activity::Running, app.pane().activity());
    committed(&mut app, 13, "runtime.step_prepared", json!({}));
    assert_eq!(Activity::WaitingForModel, app.pane().activity());
    let finished = snapshot(&app, 16, "idle", None, 0, Value::Null, None);
    app.update(Update::Refreshed(app.selection(), Ok(finished)));
    assert_eq!(Activity::Finished, app.pane().activity());
    streaming(&mut app, 15);
    assert_eq!(Activity::Finished, app.pane().activity());
    let failed = snapshot(
        &app,
        20,
        "idle",
        None,
        0,
        Value::Null,
        Some("Provider response timed out"),
    );
    app.update(Update::Refreshed(app.selection(), Ok(failed)));
    assert_eq!(Activity::Failed, app.pane().activity());
    assert_eq!(ServiceStatus::Connected, app.service_status());
}

#[test]
fn reconnect_discards_live_preview_without_changing_drafts_and_rejects_old_target() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let attached = snapshot(
        &app,
        6,
        "running",
        Some(5),
        0,
        json!({"phase":"waiting_for_model"}),
        None,
    );
    loaded(&mut app, attached, Vec::new());
    let active = app.selection();
    app.pane_mut().draft.insert("Keep this draft");
    streaming(&mut app, 7);
    app.update(Update::WatchReconnecting(
        active.clone(),
        "service observation ended".into(),
    ));
    assert_eq!(ServiceStatus::Reconnecting, app.service_status());
    assert_eq!(Activity::WaitingForModel, app.pane().activity());
    assert!(app.service_problem().unwrap().contains("observation ended"));
    assert_eq!("Keep this draft", app.pane().draft.text());
    app.update(Update::WatchConnected(active.clone()));
    app.update(Update::Observation(
        active.clone(),
        Observation::Lagged { skipped: 0 },
    ));
    assert_eq!(ServiceStatus::Connected, app.service_status());
    assert_eq!(None, app.service_problem());

    let reviewer = Address::new(app.active.session.clone(), "reviewer".into());
    app.select(reviewer, true, String::new());
    let unauthorized = || Error::Service {
        status: 401,
        code: "unauthorized".into(),
        error_message: "Sign in again".into(),
    };
    app.update(Update::WatchFailed(active, unauthorized()));
    assert_eq!(ServiceStatus::Connecting, app.service_status());
    app.update(Update::WatchFailed(app.selection(), unauthorized()));
    assert_eq!(ServiceStatus::UpdatesUnavailable, app.service_status());
    assert!(app.service_problem().unwrap().contains("Sign in again"));
}

#[test]
fn draft_confirmation_durable_queue_and_admission_are_independent() {
    let mut app = connected();
    app.select(app.active.clone(), true, String::new());
    let attached = snapshot(&app, 6, "running", Some(5), 0, Value::Null, None);
    loaded(&mut app, attached, Vec::new());
    app.pane_mut().draft.insert("Next turn");
    assert!(app.input_status().draft);
    app.send(Timing::NextTurn);
    let Some(Job::Save(command)) = app.effects.pop() else {
        panic!("send must be saved before transmission")
    };
    assert_eq!(
        Some(SubmissionStatus::Saving),
        app.input_status().submission
    );
    app.update(Update::Saved(command.clone(), Ok(())));
    app.pane_mut().draft.insert("Another idea");
    assert!(app.input_status().draft);
    assert_eq!(
        Some(SubmissionStatus::AwaitingReceipt),
        app.input_status().submission
    );
    app.update(Update::Sent(
        command.command_id.clone(),
        Ok(Acknowledgement {
            receipt: Receipt {
                command_id: command.command_id,
                event_id: 7,
                agent: "main".into(),
            },
            replayed: false,
            status: Some(InputStatus::Queued),
            warnings: vec![],
        }),
    ));
    assert_eq!(
        Some(SubmissionStatus::Queued),
        app.input_status().submission
    );
    let queued = snapshot(&app, 7, "running", Some(5), 1, Value::Null, None);
    app.update(Update::Refreshed(app.selection(), Ok(queued)));
    assert_eq!(1, app.input_status().queued);
    committed(
        &mut app,
        8,
        "runtime.operation",
        json!({"operation":"inputs_accepted","state_changed":true}),
    );
    let admitted = snapshot(&app, 8, "running", Some(5), 0, Value::Null, None);
    app.update(Update::Refreshed(app.selection(), Ok(admitted)));
    assert_eq!(0, app.input_status().queued);
    assert_eq!(
        Some(SubmissionStatus::Accepted),
        app.input_status().submission
    );
    assert!(app.input_status().draft);
}

#[test]
fn first_attachment_observes_input_already_accepted_after_queued_receipt() {
    let mut app = connected();
    app.pane_mut().draft.insert("First send");
    app.send(Timing::Steer);
    let Some(Job::Save(command)) = app.effects.pop() else {
        panic!("send must be saved before transmission")
    };
    app.update(Update::Saved(command.clone(), Ok(())));
    app.update(Update::Sent(
        command.command_id.clone(),
        Ok(Acknowledgement {
            receipt: Receipt {
                command_id: command.command_id,
                event_id: 3,
                agent: "main".into(),
            },
            replayed: false,
            status: Some(InputStatus::Queued),
            warnings: vec![],
        }),
    ));
    let attached = snapshot(&app, 6, "running", Some(4), 0, Value::Null, None);
    let input = serde_json::from_value(json!({
        "id":6,"agent":"main","type":"user","text":"First send"
    }))
    .unwrap();
    loaded(&mut app, attached, vec![input]);
    assert_eq!(
        Some(SubmissionStatus::Accepted),
        app.input_status().submission
    );
}
