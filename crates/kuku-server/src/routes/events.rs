use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kuku::event::{EventPayload, EventStore};
use kuku::session::{Observation, OutputRead, Session, SessionHistory};
use kuku_client::RewindGroup;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::{ReceiverStream, WatchStream};
use tokio_stream::Stream;

use super::rewind_history;
use super::Params;
use super::{manager, SessionTarget};
use crate::error_mapping::{ApiError, Result};
use crate::run_manager::{RunManager, TurnLookup};
use crate::wire::{
    agent_activity_frame, committed_frame, event_agent, history_entry, live_frame,
    reply_preview_frame,
};
use crate::AppState;

pub(super) const HISTORY_DEFAULT_LIMIT: usize = 40;
pub(super) const HISTORY_PAGE_BYTES: usize = 1024 * 1024;
pub(super) const HISTORY_RESPONSE_OVERHEAD: usize = 512;
pub(super) const HISTORY_SCAN_LIMIT: usize = 4096;
pub(super) const READ_PAGE_SIZE: usize = 64;
const HEARTBEAT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
pub(super) struct EventsQuery {
    workspace: PathBuf,
    after: Option<u64>,
    before: Option<u64>,
    #[serde(default)]
    reverse: bool,
    #[serde(alias = "conversation")]
    agent: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
pub(super) struct HistoryQuery {
    workspace: PathBuf,
    agent: Option<String>,
    before: Option<u64>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
pub(super) struct ObserveQuery {
    workspace: PathBuf,
    agent: Option<String>,
    after: Option<u64>,
    #[serde(default)]
    include_agent_activity: bool,
}

#[derive(Deserialize)]
pub(super) struct OutputQuery {
    workspace: PathBuf,
    agent: Option<String>,
    offset: Option<u64>,
    max_bytes: Option<usize>,
    search: Option<String>,
}

pub(super) async fn events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<EventsQuery>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let after = query.after.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);
    validate_limit(limit)?;
    let reverse = query.reverse || query.before.is_some();
    if reverse && query.after.is_some() {
        return Err(ApiError::invalid(
            "after cannot be used with before or reverse",
        ));
    }
    let events = if reverse {
        EventStore::read_page_before(&target.path, query.before, limit)?
    } else {
        EventStore::read_page(&target.path, after, limit)?
    };
    let next_after = events.last().map_or(after, |event| event.id);
    let next_before = if reverse {
        events.first().map(|event| event.id)
    } else {
        None
    };
    let filtered = events
        .iter()
        .filter(|event| {
            query.agent.as_ref().is_none_or(|agent| {
                matches!(event.payload, EventPayload::SessionCreated { .. })
                    || event_agent(&event.payload).unwrap_or("main") == agent
            })
        })
        .map(|event| json!({"id":event.id,"payload":event.payload}))
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"ok":true,"events":filtered,"next_after":next_after,"next_before":next_before}),
    ))
}

pub(super) async fn history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<HistoryQuery>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let limit = query.limit.unwrap_or(HISTORY_DEFAULT_LIMIT);
    validate_limit(limit)?;
    let cursor = EventStore::read_head(&target.path)?;
    let before = match cursor.checked_add(1) {
        Some(end) => query.before.map(|before| before.min(end)),
        None => query.before,
    };
    let (history, turns) = manager(&state).await.history(&target.path, cursor)?;
    let agent = query.agent.as_deref().unwrap_or("main");
    let (entries, rewind_groups, next_before) =
        history_page(agent, before, limit, &turns, &history)?;
    let response = json!({"ok":true,"entries":entries,"rewind_groups":rewind_groups,"next_before":next_before});
    validate_response_bytes(&response)?;
    Ok(Json(response))
}

pub(super) async fn observe(
    State(state): State<Arc<AppState>>,
    Extension(stop): Extension<watch::Receiver<bool>>,
    Path(id): Path<String>,
    Params(query): Params<ObserveQuery>,
) -> Result<Response> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    let after = query.after.unwrap_or_default();
    {
        let stopping = stop.borrow();
        if *stopping {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_draining",
                "the server is stopping",
            ));
        }
        target.require_exists()?;
        if after != 0 && after > EventStore::read_head(&target.path)? {
            return Err(ApiError::invalid(
                "observation cursor is ahead of the Session",
            ));
        }
    }
    let manager = manager(&state).await;
    let (send, read) = mpsc::channel(1);
    let follow = async move {
        if let Err(error) = follow(
            &manager,
            &target,
            query.agent.as_deref(),
            query.include_agent_activity,
            after,
            &send,
        )
        .await
        {
            let mut value = error.value();
            value["type"] = json!("error");
            let _ = write_frame(&send, value).await;
        }
    };
    let body = ObservationBody {
        frames: ReceiverStream::new(read),
        follow: Some(Box::pin(follow)),
        stopped: WatchStream::from_changes(stop.clone()),
        stop,
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(body),
    )
        .into_response())
}

pub(super) async fn output(
    State(state): State<Arc<AppState>>,
    Path((id, output_id)): Path<(String, String)>,
    Params(query): Params<OutputQuery>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let mut read = OutputRead {
        search: query.search,
        ..OutputRead::default()
    };
    if let Some(offset) = query.offset {
        read.offset = offset;
    }
    if let Some(max_bytes) = query.max_bytes {
        read.limit = max_bytes;
    }
    let agent = query.agent.as_deref().unwrap_or("main");
    let output = match manager(&state).await.open_session(&target.path) {
        Some(session) => session.read_agent_output(agent, &output_id, &read)?,
        None => Session::read_saved_agent_output(&target.path, agent, &output_id, &read)?,
    };
    Ok(Json(json!({"ok":true,"output":output})))
}

pub(super) fn validate_limit(limit: usize) -> Result<()> {
    if !(1..=256).contains(&limit) {
        return Err(ApiError::invalid("limit must be between 1 and 256"));
    }
    Ok(())
}

pub(super) fn validate_response_bytes(response: &Value) -> Result<()> {
    if serde_json::to_vec(response)
        .map_err(kuku::Error::from)?
        .len()
        > HISTORY_PAGE_BYTES
    {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "history_page_too_large",
            "history page exceeds the response byte limit",
        ));
    }
    Ok(())
}

fn history_page(
    agent: &str,
    mut before: Option<u64>,
    limit: usize,
    turns: &TurnLookup,
    history: &SessionHistory,
) -> Result<(Vec<Value>, Vec<RewindGroup>, Option<u64>)> {
    let mut entries = Vec::new();
    let mut group_entries = Vec::new();
    let mut bytes = 0;
    let mut scanned = 0;
    let mut items = 0;
    let max_item_bytes = HISTORY_PAGE_BYTES.saturating_sub(HISTORY_RESPONSE_OVERHEAD);
    'pages: loop {
        let page = history.page_before(before, READ_PAGE_SIZE)?;
        if page.is_empty() {
            before = None;
            break;
        }
        for event in page.into_iter().rev() {
            if let EventPayload::MessageUser { turn, .. } = &event.payload {
                for rewind in history
                    .rewinds()
                    .iter()
                    .filter(|rewind| rewind.target_event_id == event.id && rewind.agent == agent)
                {
                    let group = rewind_history::summary(history, turns, rewind, *turn)?;
                    let size = serde_json::to_vec(&group).map_err(kuku::Error::from)?.len();
                    if size > max_item_bytes {
                        return Err(ApiError::new(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "history_entry_too_large",
                            "history item exceeds the response byte limit",
                        ));
                    }
                    if items > 0 && bytes + size > max_item_bytes {
                        break 'pages;
                    }
                    bytes += size;
                    group_entries.push(group);
                    items += 1;
                    if items == limit {
                        before = Some(event.id);
                        break 'pages;
                    }
                }
            }
            let entry = (history.includes(&event)
                && event_agent(&event.payload).unwrap_or("main") == agent)
                .then(|| history_entry(&event, turns))
                .flatten();
            if let Some(entry) = entry {
                let size = serde_json::to_vec(&entry).map_err(kuku::Error::from)?.len();
                if size > max_item_bytes {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "history_entry_too_large",
                        "history item exceeds the response byte limit",
                    ));
                }
                if items > 0 && bytes + size > max_item_bytes {
                    break 'pages;
                }
                bytes += size;
                entries.push(entry);
                items += 1;
                if items == limit {
                    before = Some(event.id);
                    break 'pages;
                }
            }
            before = Some(event.id);
            scanned += 1;
            if scanned >= HISTORY_SCAN_LIMIT {
                break 'pages;
            }
        }
    }
    entries.reverse();
    group_entries.reverse();
    add_tool_names(&mut entries);
    Ok((entries, group_entries, before.filter(|id| *id > 1)))
}

pub(super) fn add_tool_names(entries: &mut [Value]) {
    let mut tools = HashMap::new();
    for entry in entries {
        if let (Some(turn), Some(call)) = (entry["turn"].as_u64(), entry["tool_call_id"].as_str()) {
            if let Some(tool) = entry["tool"].as_str() {
                tools.insert((turn, call.to_owned()), tool.to_owned());
            } else if let Some(tool) = tools.get(&(turn, call.to_owned())) {
                entry["tool"] = json!(tool);
            }
        }
    }
}

type Frame = std::result::Result<String, Infallible>;
type FrameWriter = mpsc::Sender<Frame>;

struct ObservationBody<F> {
    frames: ReceiverStream<Frame>,
    follow: Option<Pin<Box<F>>>,
    stopped: WatchStream<bool>,
    stop: watch::Receiver<bool>,
}

impl<F: Future<Output = ()>> Stream for ObservationBody<F> {
    type Item = Frame;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let stop_event = Pin::new(&mut this.stopped).poll_next(context);
        // Hold the stop watch's read guard while the follower may touch the journal.
        let stopping = this.stop.borrow();
        if *stopping || matches!(stop_event, Poll::Ready(None)) {
            this.follow = None;
            return Poll::Ready(None);
        }
        if let Some(follow) = &mut this.follow {
            if follow.as_mut().poll(context).is_ready() {
                this.follow = None;
            }
        }
        Pin::new(&mut this.frames).poll_next(context)
    }
}

async fn follow(
    manager: &RunManager,
    target: &SessionTarget,
    agent: Option<&str>,
    include_agent_activity: bool,
    mut after: u64,
    send: &FrameWriter,
) -> Result<()> {
    let mut changes = manager.changes();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if let Some(session) = manager.open_session(&target.path) {
            let turns = TurnLookup::Live(session.clone());
            let mut observer = match agent.filter(|_| include_agent_activity) {
                Some(address) => session.observe_agent_with_activity(after, address),
                None => session.observe(after),
            };
            loop {
                let observation = tokio::select! {
                    _ = send.closed() => return Ok(()),
                    _ = ping.tick() => {
                        if !write_frame(send, json!({"type":"heartbeat"})).await {
                            return Ok(());
                        }
                        continue;
                    }
                    item = observer.next() => item?,
                };
                let Some(observation) = observation else {
                    break;
                };
                let frame = match observation {
                    Observation::Committed(event) => {
                        after = event.id;
                        Some(committed_frame(
                            &event,
                            agent,
                            include_agent_activity,
                            &turns,
                        ))
                    }
                    Observation::Live {
                        agent: source,
                        event,
                        execution_id,
                        after,
                        turn,
                    } => {
                        if agent.is_none_or(|agent| agent == source) {
                            live_frame(
                                &event,
                                &source,
                                execution_id,
                                turn,
                                after,
                                session.turn_number(&source, turn),
                                include_agent_activity,
                            )
                        } else if include_agent_activity {
                            agent_activity_frame(&event, &source, execution_id, turn, after)
                        } else {
                            None
                        }
                    }
                    Observation::Lagged { skipped } => {
                        Some(json!({"type":"lagged","skipped":skipped}))
                    }
                    Observation::ReplyPreview {
                        agent,
                        after,
                        reply,
                    } => {
                        let turn_number = session.turn_number(&agent, reply.turn);
                        Some(reply_preview_frame(&agent, after, &reply, turn_number))
                    }
                };
                if let Some(frame) = frame {
                    if !write_frame(send, frame).await {
                        return Ok(());
                    }
                }
            }
            tokio::task::yield_now().await;
        } else {
            target.require_exists()?;
            let events = EventStore::read_page(&target.path, after, READ_PAGE_SIZE)?;
            if let Some(last) = events.last() {
                let turns = manager.turn_lookup(&target.path, last.id)?;
                for event in events {
                    after = event.id;
                    let frame = committed_frame(&event, agent, include_agent_activity, &turns);
                    if !write_frame(send, frame).await {
                        return Ok(());
                    }
                }
                continue;
            }
            tokio::select! {
                _ = send.closed() => return Ok(()),
                _ = changes.changed() => {}
                _ = ping.tick() => {
                    if !write_frame(send, json!({"type":"heartbeat"})).await {
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn write_frame(send: &FrameWriter, frame: Value) -> bool {
    send.send(Ok(format!("{frame}\n"))).await.is_ok()
}

#[cfg(test)]
mod tests {
    use kuku::event::{EventPayload, StoredEvent};
    use serde_json::json;

    use super::{validate_response_bytes, HISTORY_PAGE_BYTES};

    fn read_history(events: &[StoredEvent]) -> (tempfile::TempDir, kuku::session::SessionHistory) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.jsonl");
        let mut store = kuku::event::EventStore::open(&path).unwrap();
        for event in events {
            store.append(event.payload.clone()).unwrap();
        }
        drop(store);
        let history = kuku::session::SessionHistory::read(&path).unwrap();
        (root, history)
    }

    fn user_event(id: u64, agent: &str, text: &str) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::MessageUser {
                ts: "t".into(),
                conversation: agent.into(),
                turn: id,
                text: text.into(),
                from: None,
                via_tool_call_id: None,
                job_delivery: false,
            },
        }
    }

    #[test]
    fn committed_history_hides_only_the_rewound_agent_interval() {
        let events = vec![
            user_event(1, "main", "selected"),
            user_event(2, "main", "later tail"),
            StoredEvent {
                id: 3,
                payload: EventPayload::Unknown(json!({
                    "id":3,
                    "kind":"runtime.operation",
                    "record":{
                        "operation":"message_rewind",
                        "address":"main",
                        "target_event_id":1,
                    }
                })),
            },
            user_event(4, "main", "new input"),
            user_event(5, "reviewer", "other Agent"),
        ];

        let (_root, history) = read_history(&events);
        let included: Vec<_> = events.iter().map(|event| history.includes(event)).collect();
        assert_eq!(included, [false, false, true, true, true]);
    }

    #[test]
    fn rewind_archives_compaction_progress_and_its_checkpoint() {
        let events = vec![
            user_event(1, "main", "selected"),
            StoredEvent {
                id: 2,
                payload: EventPayload::Unknown(json!({
                    "kind":"runtime.compaction_started", "agent":"main", "turn":1, "request_id":"compact-1",
                })),
            },
            StoredEvent {
                id: 3,
                payload: EventPayload::Unknown(json!({
                    "kind":"runtime.compaction_finished", "agent":"main", "turn":1, "request_id":"compact-1",
                    "status":"completed", "summary":"Retained in rewind", "elapsed_ms":2000,
                    "before_bytes":10000, "after_bytes":5000, "checkpointed_messages":6,
                })),
            },
            StoredEvent {
                id: 4,
                payload: EventPayload::Unknown(json!({
                    "kind":"runtime.operation", "record":{
                        "operation":"message_rewind", "address":"main", "target_event_id":1,
                    },
                })),
            },
        ];
        let turns = crate::run_manager::TurnLookup::Closed(std::sync::Arc::default());
        let (_root, history) = read_history(&events);
        let group =
            super::rewind_history::summary(&history, &turns, &history.rewinds()[0], 1).unwrap();
        assert_eq!(3, group.entry_count);
        assert!(!history.includes(&events[1]) && !history.includes(&events[2]));
        let entry = crate::wire::history_entry_for_agent(&events[2], &turns, "main").unwrap();
        assert_eq!(entry["type"], "compaction");
        assert_eq!(entry["text"], "Retained in rewind");
    }

    #[test]
    fn history_response_byte_cap_includes_the_serialized_envelope() {
        let response = json!({
            "ok": true,
            "entries": [],
            "rewind_groups": [],
            "next_before": null,
            "payload": "x".repeat(HISTORY_PAGE_BYTES),
        });

        assert!(validate_response_bytes(&response).is_err());
    }
}
