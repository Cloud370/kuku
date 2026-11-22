use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kuku::event::EventPayload;
use kuku_client::{
    Acknowledgement, Action, AgentProgress, ApiError, Client, Command, EventPage, HistoryKind,
    HistoryPage, InputDelivery, InputStatus, JobStatus, Observation, PendingStore,
    RetryFailureKind, RetryPhase, RetryStopReason, SubscriptionUpdate, Target, Timing,
};
use serde::Deserialize;
use serde_json::{json, Value};

const PROTOCOL: &str =
    include_str!("../../kuku-server/tests/common/fixtures/interactive-protocol.json");

#[tokio::test]
async fn ledger_events_decode_known_and_unknown_payloads_from_the_server_envelope() {
    #[derive(Deserialize)]
    struct LedgerQuery {
        workspace: PathBuf,
        agent: String,
        after: u64,
        limit: usize,
    }

    async fn events(Path(session): Path<String>, Query(query): Query<LedgerQuery>) -> Json<Value> {
        assert_eq!(target().session_id, session);
        assert_eq!(target().workspace, query.workspace);
        assert_eq!("main", query.agent);
        assert_eq!(8, query.after);
        assert_eq!(2, query.limit);
        Json(json!({"events":[
            {"id":9,"payload":EventPayload::MessageUser {
                ts:"2026-11-20T00:00:00Z".into(),conversation:"main".into(),turn:7,
                text:"retained input".into(),from:None,via_tool_call_id:None,
                job_delivery: false,
            }},
            {"id":10,"payload":EventPayload::Unknown(json!({
                "id":10,"kind":"runtime.step_prepared","agent":"main","turn":7,
                "request_id":"request-7","segment":6,"connection":"actual-provider",
                "model":"actual-model",
            }))}
        ],"next_after":12}))
    }

    let (client, _server) = start(Router::new().route("/sessions/{id}/events", get(events))).await;
    let target = target();
    let page = client
        .events(
            &target.workspace,
            &target.session_id,
            Some(&target.agent),
            8,
            2,
        )
        .await
        .unwrap();
    assert_eq!(12, page.next_after);
    assert_eq!(9, page.events[0].id);
    assert!(
        matches!(&page.events[0].payload, EventPayload::MessageUser {text, turn:7, ..}
        if text == "retained input")
    );
    assert_eq!(10, page.events[1].id);
    let EventPayload::Unknown(record) = &page.events[1].payload else {
        panic!("runtime event stays unknown to the SDK codec");
    };
    assert_eq!("runtime.step_prepared", record["kind"]);
    assert_eq!("request-7", record["request_id"]);
    assert_eq!(6, record["segment"]);
    assert_eq!(10, record["id"]);
}

#[test]
fn ledger_events_reject_non_wire_shapes_and_conflicting_event_ids() {
    for event in [
        json!({"id":9,"kind":"runtime.step_prepared"}),
        json!({"id":9,"payload":{"id":10,"kind":"runtime.step_prepared"}}),
        json!({"id":9,"payload":"not an event"}),
    ] {
        assert!(serde_json::from_value::<EventPage>(json!({
            "events":[event],"next_after":9
        }))
        .is_err());
    }
}

#[tokio::test]
async fn background_job_requests_use_the_selected_workspace_and_agent() {
    async fn list(Path(session): Path<String>, Query(query): Query<JobQuery>) -> Json<Value> {
        assert_eq!(session, target().session_id);
        check_job_query(query);
        Json(json!({"jobs": [job("running")]}))
    }

    async fn show(
        Path((session, id)): Path<(String, String)>,
        Query(query): Query<JobQuery>,
    ) -> Json<Value> {
        check_job_target(session, id, query);
        Json(job("running"))
    }

    async fn cancel(
        Path((session, id)): Path<(String, String)>,
        Query(query): Query<JobQuery>,
    ) -> Json<Value> {
        check_job_target(session, id, query);
        Json(job("cancelled"))
    }

    fn check_job_target(session: String, id: String, query: JobQuery) {
        assert_eq!(session, target().session_id);
        assert_eq!(id, "job-1");
        check_job_query(query);
    }

    fn check_job_query(query: JobQuery) {
        assert_eq!(query.workspace, target().workspace);
        assert_eq!(query.agent, "reviewer");
    }

    fn job(status: &str) -> Value {
        json!({
            "id": "job-1", "agent": "reviewer", "turn": 4, "tool_call_id": "call-1",
            "tool": "run_command", "status": status, "summary": "Run tests", "started_at": 6,
        })
    }

    #[derive(Deserialize)]
    struct JobQuery {
        workspace: PathBuf,
        agent: String,
    }

    let (client, _server) = start(
        Router::new()
            .route("/sessions/{id}/jobs", get(list))
            .route("/sessions/{id}/jobs/{job_id}", get(show).delete(cancel)),
    )
    .await;
    let target = Target {
        agent: "reviewer".into(),
        ..target()
    };
    let jobs = client.jobs(&target).await.unwrap();
    assert_eq!(jobs.len(), 1);
    let listed = &jobs[0];
    assert_eq!(listed.status, JobStatus::Running);
    assert_eq!(*listed, client.job(&target, &listed.id).await.unwrap());
    let cancelled = client.cancel_job(&target, &listed.id).await.unwrap();
    assert_eq!(cancelled.id, listed.id);
    assert_eq!(cancelled.status, JobStatus::Cancelled);
}

#[test]
fn shared_protocol_fixture_decodes_history_receipts_observation_and_errors() {
    let fixture: Value = serde_json::from_str(PROTOCOL).unwrap();
    let receipt: Acknowledgement = serde_json::from_value(fixture["receipt"].clone()).unwrap();
    assert_eq!("send-example", receipt.receipt.command_id);
    assert_eq!(Some(InputStatus::Queued), receipt.status);
    let history: HistoryPage = serde_json::from_value(fixture["history"].clone()).unwrap();
    assert_eq!(HistoryKind::ToolResult, history.entries[2].kind);
    assert_eq!("out-1", history.entries[2].details["output_id"]);
    let frames: Vec<Observation> = serde_json::from_value(fixture["observation"].clone()).unwrap();
    assert!(
        matches!(&frames[0], Observation::Live { agent, execution_id: 5, .. } if agent == "main")
    );
    assert!(
        matches!(&frames[1], Observation::Committed { id: 22, entry: Some(entry), .. } if entry.text.as_deref() == Some("I found the workspace."))
    );
    assert!(matches!(
        frames[2],
        Observation::Committed {
            id: 24,
            state_changed: true,
            ..
        }
    ));
    assert!(matches!(frames[3], Observation::Lagged { skipped: 25 }));
    assert!(matches!(frames[4], Observation::Heartbeat));
    let error: ApiError = serde_json::from_value(fixture["error"].clone()).unwrap();
    assert_eq!("command_conflict", error.code);
}

#[test]
fn legacy_history_page_defaults_to_empty_rewind_groups() {
    let history: HistoryPage = serde_json::from_value(json!({
        "entries": [],
        "next_before": null
    }))
    .unwrap();

    assert!(history.rewind_groups.is_empty());
}

#[test]
fn retry_history_and_snapshot_decode_phases_attempts_and_unknown_values() {
    let history: HistoryPage = serde_json::from_value(json!({
        "entries":[{
            "id":31,"agent":"main","turn":9,"type":"retry","text":null,
            "retry":{
                "source":"runtime.model_retry","phase":"waiting","attempt":2,
                "failed_attempt":1,"max_attempts":4,"failed_kind":"rate_limited",
                "request_id":"req-1","delay_ms":5000,"ts":"2025-01-01T00:00:00Z",
                "will_retry":true,"status":429
            }
        }],"next_before":null
    }))
    .unwrap();
    assert_eq!(HistoryKind::Retry, history.entries[0].kind);
    let retry: kuku_client::RetryMetadata =
        serde_json::from_value(history.entries[0].details["retry"].clone()).unwrap();
    assert_eq!(RetryPhase::Waiting, retry.phase);
    assert_eq!(Some(2), retry.attempt);
    assert_eq!(Some(1), retry.failed_attempt);
    assert_eq!(Some(4), retry.max_attempts);
    assert_eq!(Some(RetryFailureKind::RateLimited), retry.failed_kind);
    assert_eq!(Some("req-1"), retry.request_id.as_deref());
    assert_eq!(Some(429), retry.status);
    let legacy: kuku_client::RetryMetadata = serde_json::from_value(json!({
        "source":"runtime.model_retry","attempt":2
    }))
    .unwrap();
    assert_eq!(RetryPhase::Started, legacy.phase);
    assert!(legacy.will_retry.is_none());

    let stopped: AgentProgress = serde_json::from_value(json!({
        "phase":"retry_stopped",
        "retry":{
            "source":"runtime.model_retry","phase":"future_phase","attempt":3,
            "failed_kind":"future_failure","will_retry":false,
            "stop_reason":"future_policy"
        }
    }))
    .unwrap();
    assert!(matches!(stopped, AgentProgress::RetryStopped { retry }
        if retry.phase == RetryPhase::Unknown
            && retry.failed_kind == Some(RetryFailureKind::Unknown)
            && retry.stop_reason == Some(RetryStopReason::Unknown)
            && retry.will_retry == Some(false)));

    let undecided: AgentProgress = serde_json::from_value(json!({
        "phase":"retry_decision_unknown",
        "retry":{"source":"runtime.model_retry","phase":"future_phase","attempt":3}
    }))
    .unwrap();
    assert!(
        matches!(undecided, AgentProgress::RetryDecisionUnknown { retry }
        if retry.phase == RetryPhase::Unknown && retry.will_retry.is_none())
    );
}

#[test]
fn shared_protocol_preserves_the_command_id_for_each_queue_transition() {
    let fixture: Value = serde_json::from_str(PROTOCOL).unwrap();
    let action = Action::ReplaceQueued {
        input_command_id: "send-example".into(),
        text: "Inspect the two modified files".into(),
    };
    assert_eq!(
        fixture["queued_replacement"],
        serde_json::to_value(&action).unwrap()
    );
    assert_eq!(
        action,
        serde_json::from_value(fixture["queued_replacement"].clone()).unwrap()
    );
    let frames: Vec<Observation> =
        serde_json::from_value(fixture["queue_observation"].clone()).unwrap();
    let delivery: Vec<_> = frames
        .into_iter()
        .map(|frame| match frame {
            Observation::Committed {
                delivery: Some(delivery),
                ..
            } => *delivery,
            other => panic!("unexpected queue observation: {other:?}"),
        })
        .collect();
    assert_eq!(
        vec![
            InputDelivery::Replaced {
                input_id: "send-example".into(),
                replacement_id: "edited-example".into()
            },
            InputDelivery::Withdrawn {
                input_id: "other-example".into()
            },
            InputDelivery::Accepted {
                input_ids: vec!["edited-example".into()]
            },
        ],
        delivery
    );
}

#[test]
fn message_rewind_protocol_preserves_exact_event_text_and_restore_opt_in() {
    let action = Action::RewindMessage {
        target_event_id: 41,
        text: "keep this as an unsent draft".into(),
        restore_files: false,
    };
    let encoded = serde_json::to_value(&action).unwrap();
    assert_eq!("rewind_message", encoded["type"]);
    assert_eq!(41, encoded["target_event_id"]);
    assert_eq!("keep this as an unsent draft", encoded["text"]);
    assert_eq!(false, encoded["restore_files"]);
    assert_eq!(
        action,
        serde_json::from_value(encoded).expect("rewind action round-trips")
    );
}

#[tokio::test]
async fn missing_acknowledgement_reopens_exact_command_and_checks_receipt_before_replaying() {
    let calls = Arc::new(SubmitCalls::default());
    let router = Router::new()
        .route("/sessions/{id}/commands", post(submit))
        .route("/sessions/{id}/receipts/{command_id}", get(receipt))
        .with_state(calls.clone());
    let (client, _server) = start(router).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending");
    let store = PendingStore::open(&path).unwrap();
    let command = Command {
        target: target(),
        command_id: "send-example".into(),
        action: Action::Send {
            text: "Inspect the workspace".into(),
            timing: Timing::Steer,
            values: Vec::new(),
            bootstrap_skill: None,
            create: Some(Box::default()),
            config_patch: None,
            prompts_dir: None,
            create_agent_if_missing: false,
        },
    };
    assert!(store.submit(&client, &command).await.is_err());
    drop(store);

    let store = PendingStore::open(&path).unwrap();
    let records = store.list().unwrap();
    assert_eq!(vec![command.clone()], records);
    let acknowledgement = store
        .reconcile(&client, &records[0])
        .await
        .unwrap()
        .unwrap();
    assert!(acknowledgement.replayed);
    assert_eq!(4, acknowledgement.receipt.event_id);
    assert_eq!(1, calls.deliveries.load(Ordering::SeqCst));
    assert_eq!(1, calls.lookups.load(Ordering::SeqCst));
    assert!(store.list().unwrap().is_empty());

    let missing = Command {
        command_id: "receipt-absent".into(),
        ..command
    };
    store.save(&missing).unwrap();
    assert_eq!(None, store.reconcile(&client, &missing).await.unwrap());
    assert_eq!(1, calls.deliveries.load(Ordering::SeqCst));
    assert_eq!(vec![missing], store.list().unwrap());
}

#[tokio::test]
async fn rejected_queued_replacement_remains_recoverable_after_reopening_the_store() {
    let router = Router::new().route(
        "/sessions/{id}/commands",
        post(|| async {
            (
                StatusCode::CONFLICT,
                Json(json!({"ok":false,"code":"input_not_queued","message":"Input already admitted"})),
            )
        }),
    );
    let (client, _server) = start(router).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending");
    let store = PendingStore::open(&path).unwrap();
    let command = Command {
        target: target(),
        command_id: "edit-example".into(),
        action: Action::ReplaceQueued {
            input_command_id: "original-example".into(),
            text: "Keep this edited text\nincluding the second line".into(),
        },
    };
    let error = store.submit(&client, &command).await.unwrap_err();
    assert_eq!(Some("input_not_queued"), error.code());
    assert!(error.definitively_rejected());
    drop(store);

    let restored = PendingStore::open(&path).unwrap();
    assert_eq!(vec![command.clone()], restored.list().unwrap());
    restored.remove(&command).unwrap();
    assert!(restored.list().unwrap().is_empty());
}

#[tokio::test]
async fn observation_reconnects_with_last_committed_id_and_deduplicates_scanned_frames() {
    let subscriptions = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/sessions/{id}/observe", get(observe))
        .with_state(subscriptions.clone());
    let (client, _server) = start(router).await;
    let target = target();
    let mut stream = client.subscribe(&target, 20);
    assert!(matches!(
        stream.next().await.unwrap(),
        Observation::Committed { id: 24, .. }
    ));
    assert!(matches!(
        stream.next().await.unwrap(),
        Observation::Lagged { .. }
    ));
    assert!(matches!(
        stream.next().await.unwrap(),
        Observation::Committed { id: 30, .. }
    ));
    assert_eq!(30, stream.cursor());
    assert_eq!(&target, stream.target());
    assert_eq!(2, subscriptions.load(Ordering::SeqCst));
}

#[tokio::test]
async fn interactive_subscription_reports_connection_loss_before_resuming() {
    let router = Router::new()
        .route("/sessions/{id}/observe", get(observe))
        .with_state(Arc::new(AtomicUsize::new(0)));
    let (client, _server) = start(router).await;
    let mut stream = client.subscribe(&target(), 20);
    assert_eq!(
        SubscriptionUpdate::Connected,
        stream.next_update().await.unwrap()
    );
    assert!(matches!(
        stream.next_update().await.unwrap(),
        SubscriptionUpdate::Observation(Observation::Committed { id: 24, .. })
    ));
    assert!(matches!(
        stream.next_update().await.unwrap(),
        SubscriptionUpdate::Reconnecting(reason) if reason.contains("observation ended")
    ));
    assert_eq!(24, stream.cursor());
    assert_eq!(
        SubscriptionUpdate::Connected,
        stream.next_update().await.unwrap()
    );
    assert!(matches!(
        stream.next_update().await.unwrap(),
        SubscriptionUpdate::Observation(Observation::Lagged { skipped: 0 })
    ));
    assert!(matches!(
        stream.next_update().await.unwrap(),
        SubscriptionUpdate::Observation(Observation::Committed { id: 30, .. })
    ));
}

#[tokio::test]
async fn authorization_error_does_not_report_a_transport_reconnect() {
    let router = Router::new().route(
        "/sessions/{id}/observe",
        get(|| async {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"ok":false,"code":"unauthorized","message":"Sign in"})),
            )
        }),
    );
    let (client, _server) = start(router).await;
    let mut stream = client.subscribe(&target(), 20);
    assert!(matches!(
        stream.next_update().await,
        Err(kuku_client::Error::Service { status: 401, .. })
    ));
}

#[derive(Default)]
struct SubmitCalls {
    deliveries: AtomicUsize,
    lookups: AtomicUsize,
}

#[derive(Deserialize)]
struct SubmitQuery {
    #[serde(default)]
    replay_only: bool,
}

async fn submit(
    State(calls): State<Arc<SubmitCalls>>,
    Query(query): Query<SubmitQuery>,
    Json(data): Json<Value>,
) -> Response {
    let reference: Value = serde_json::from_str(PROTOCOL).unwrap();
    for key in ["workspace", "agent", "command_id", "type", "text", "timing"] {
        assert_eq!(reference["first_send"][key], data[key]);
    }
    assert!(data["create"].is_object());
    if query.replay_only {
        assert_eq!(1, calls.lookups.load(Ordering::SeqCst));
        let mut acknowledgement = reference["receipt"].clone();
        acknowledgement["replayed"] = json!(true);
        Json(acknowledgement).into_response()
    } else {
        calls.deliveries.fetch_add(1, Ordering::SeqCst);
        Response::new(Body::from_stream(tokio_stream::iter([Err::<
            BytesForTest,
            _,
        >(
            std::io::Error::other("lost acknowledgement"),
        )])))
    }
}

type BytesForTest = axum::body::Bytes;

async fn receipt(
    State(calls): State<Arc<SubmitCalls>>,
    Path((_, command_id)): Path<(String, String)>,
) -> impl IntoResponse {
    calls.lookups.fetch_add(1, Ordering::SeqCst);
    if command_id == "receipt-absent" {
        let problem = json!({"ok":false,"code":"receipt_not_found","message":"Command has no committed receipt"});
        return (StatusCode::NOT_FOUND, Json(problem));
    }
    let fixture: Value = serde_json::from_str(PROTOCOL).unwrap();
    (StatusCode::OK, Json(fixture["receipt"].clone()))
}

#[derive(Deserialize)]
struct ObserveQuery {
    after: u64,
    agent: String,
}

async fn observe(
    State(subscriptions): State<Arc<AtomicUsize>>,
    Query(query): Query<ObserveQuery>,
) -> impl IntoResponse {
    assert_eq!("main", query.agent);
    let next = subscriptions.fetch_add(1, Ordering::SeqCst);
    let duplicate = "{\"type\":\"committed\",\"id\":24,\"kind\":\"tool.result\"}\n";
    let text = if next == 0 {
        assert_eq!(20, query.after);
        format!("{duplicate}{{\"type\":\"live\"")
    } else {
        assert_eq!(24, query.after);
        format!("{duplicate}{{\"type\":\"committed\",\"id\":30,\"kind\":\"turn.completed\"}}\n")
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        text,
    )
}

fn target() -> Target {
    Target {
        workspace: PathBuf::from("/work/kuku"),
        session_id: "session-1".into(),
        agent: "main".into(),
    }
}

struct Server(tokio::task::JoinHandle<()>);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn start(router: Router) -> (Client, Server) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (
        Client::remote(&endpoint, "integration-fixture").unwrap(),
        Server(server),
    )
}
