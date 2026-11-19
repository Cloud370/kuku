use kuku_client::HistoryPage;
use serde_json::json;

use super::{connected, observe};
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
            next_before: Some(5),
        }),
    ));

    assert_eq!(Some(HistoryRequest::Recent), app.pane().history_in_flight);
    assert!(app.effects.iter().any(|job| matches!(
        job,
        Job::History(target, HistoryRequest::Recent) if *target == selection
    )));
}
