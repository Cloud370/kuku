use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use kuku::event::{EventPayload, EventStore};
use kuku::session::{Observation, OutputRead, Session};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::Params;
use super::{manager, SessionTarget};
use crate::error_mapping::{ApiError, Result};
use crate::run_manager::{RunManager, TurnLookup};
use crate::wire::{agent_activity_frame, committed_frame, event_agent, history_entry, live_frame};
use crate::AppState;

const HISTORY_DEFAULT_LIMIT: usize = 40;
const HISTORY_PAGE_BYTES: usize = 1024 * 1024;
const HISTORY_SCAN_LIMIT: usize = 4096;
const READ_PAGE_SIZE: usize = 64;
const HEARTBEAT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
pub(super) struct EventsQuery {
    workspace: PathBuf,
    after: Option<u64>,
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
    let events = EventStore::read_page(&target.path, after, limit)?;
    let next_after = events.last().map_or(after, |event| event.id);
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
        json!({"ok":true,"events":filtered,"next_after":next_after}),
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
    let turns = manager(&state).await.turn_lookup(&target.path)?;
    let (entries, next_before) = history_page(
        &target.path,
        query.agent.as_deref().unwrap_or("main"),
        query.before,
        limit,
        &turns,
    )?;
    Ok(Json(
        json!({"ok":true,"entries":entries,"next_before":next_before}),
    ))
}

pub(super) async fn observe(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Params(query): Params<ObserveQuery>,
) -> Result<Response> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let after = query.after.unwrap_or_default();
    if after != 0 && after > EventStore::read_head(&target.path)? {
        return Err(ApiError::invalid(
            "observation cursor is ahead of the Session",
        ));
    }
    let manager = manager(&state).await;
    let (send, read) = mpsc::channel(1);
    tokio::spawn(async move {
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
    });
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(ReceiverStream::new(read)),
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
    let output = Session::read_saved_output(&target.path, &output_id, &read)?;
    Ok(Json(json!({"ok":true,"output":output})))
}

fn validate_limit(limit: usize) -> Result<()> {
    if !(1..=256).contains(&limit) {
        return Err(ApiError::invalid("limit must be between 1 and 256"));
    }
    Ok(())
}

fn history_page(
    path: &std::path::Path,
    agent: &str,
    mut before: Option<u64>,
    limit: usize,
    turns: &TurnLookup,
) -> Result<(Vec<Value>, Option<u64>)> {
    let mut entries = Vec::new();
    let mut bytes = 0;
    let mut scanned = 0;
    'pages: loop {
        let page = EventStore::read_page_before(path, before, READ_PAGE_SIZE)?;
        if page.is_empty() {
            before = None;
            break;
        }
        for event in page.into_iter().rev() {
            let entry = (event_agent(&event.payload).unwrap_or("main") == agent)
                .then(|| history_entry(&event, |address, turn| turns.number(address, turn)))
                .flatten();
            if let Some(entry) = entry {
                let size = serde_json::to_vec(&entry).map_err(kuku::Error::from)?.len();
                if !entries.is_empty() && bytes + size > HISTORY_PAGE_BYTES {
                    break 'pages;
                }
                bytes += size;
                entries.push(entry);
            }
            before = Some(event.id);
            scanned += 1;
            if entries.len() == limit || scanned >= HISTORY_SCAN_LIMIT {
                break 'pages;
            }
        }
    }
    entries.reverse();
    add_tool_names(&mut entries);
    Ok((entries, before.filter(|id| *id > 1)))
}

fn add_tool_names(entries: &mut [Value]) {
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

type FrameWriter = mpsc::Sender<std::result::Result<String, Infallible>>;

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
            let mut observer = session.observe(after);
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
                            |address, turn| session.turn_number(address, turn),
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
            if !events.is_empty() {
                let turns = manager.turn_lookup(&target.path)?;
                for event in events {
                    after = event.id;
                    let frame =
                        committed_frame(&event, agent, include_agent_activity, |address, turn| {
                            turns.number(address, turn)
                        });
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
