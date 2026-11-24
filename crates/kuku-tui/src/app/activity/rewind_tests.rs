use kuku_client::{Error, HistoryPage, Observation};
use serde_json::json;

use super::{Activity, ServiceStatus, connected, loaded, observe, snapshot, streaming};
use crate::app::Address;
use crate::jobs::{Job, Update};
use crate::transcript::HistoryRequest;

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
            rewind_groups: Vec::new(),
            next_before: Some(5),
        }
        .into()),
    ));

    assert_eq!(Some(HistoryRequest::Recent), app.pane().history_in_flight);
    assert!(app.effects.iter().any(|job| matches!(
        job,
        Job::History(target, HistoryRequest::Recent) if *target == selection
    )));
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
