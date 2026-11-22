use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use kuku::event::EventStore;
use kuku::session::{MessageRewind, SessionHistory};
use kuku_client::RewindGroup;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{events, manager, Params, SessionTarget};
use crate::error_mapping::{ApiError, Result};
use crate::run_manager::TurnLookup;
use crate::wire::history_entry_for_agent;
use crate::AppState;

#[derive(Deserialize)]
pub(super) struct Query {
    workspace: std::path::PathBuf,
    agent: String,
    before: Option<u64>,
    limit: Option<usize>,
}

pub(super) fn summary(
    history: &SessionHistory,
    turns: &TurnLookup,
    rewind: &MessageRewind,
    turn: u64,
) -> Result<RewindGroup> {
    let mut before = None;
    let mut entry_count = 0;
    loop {
        let (events, next) = history.rewind_page(rewind, before, events::READ_PAGE_SIZE)?;
        entry_count += events
            .iter()
            .filter(|event| history_entry_for_agent(event, turns, &rewind.agent).is_some())
            .count();
        before = next;
        if before.is_none() {
            break;
        }
    }
    Ok(RewindGroup {
        rewind_event_id: rewind.event_id,
        target_event_id: rewind.target_event_id,
        agent: rewind.agent.clone(),
        turn_number: turns.number(&rewind.agent, turn),
        entry_count,
    })
}

pub(super) async fn history(
    State(state): State<Arc<AppState>>,
    AxumPath((id, rewind_event_id)): AxumPath<(String, u64)>,
    Params(query): Params<Query>,
) -> Result<Json<Value>> {
    let target = SessionTarget::new(&state, &query.workspace, &id)?;
    target.require_exists()?;
    let limit = query.limit.unwrap_or(events::HISTORY_DEFAULT_LIMIT);
    events::validate_limit(limit)?;
    let head = EventStore::read_head(&target.path)?;
    let (history, turns) = manager(&state).await.history(&target.path, head)?;
    let rewind = history
        .rewinds()
        .iter()
        .find(|item| item.event_id == rewind_event_id && item.agent == query.agent)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "rewind_group_not_found",
                "rewind group not found for this Agent",
            )
        })?;
    let (entries, next_before) = group_page(&history, rewind, query.before, limit, &turns)?;
    let response = json!({"ok":true,"entries":entries,"next_before":next_before});
    events::validate_response_bytes(&response)?;
    Ok(Json(response))
}

fn group_page(
    history: &SessionHistory,
    rewind: &MessageRewind,
    mut before: Option<u64>,
    limit: usize,
    turns: &TurnLookup,
) -> Result<(Vec<Value>, Option<u64>)> {
    let mut entries = Vec::new();
    let mut bytes = 0;
    let max_item_bytes =
        events::HISTORY_PAGE_BYTES.saturating_sub(events::HISTORY_RESPONSE_OVERHEAD);
    let mut oldest = None;
    let next_before = 'pages: loop {
        let (page, next) = history.rewind_page(rewind, before, events::READ_PAGE_SIZE)?;
        for event in page.iter().rev() {
            let Some(entry) = history_entry_for_agent(event, turns, &rewind.agent) else {
                continue;
            };
            if entries.len() == limit {
                break 'pages oldest;
            }
            let size = serde_json::to_vec(&entry).map_err(kuku::Error::from)?.len();
            if size > max_item_bytes {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "history_entry_too_large",
                    "history item exceeds the response byte limit",
                ));
            }
            if !entries.is_empty() && bytes + size > max_item_bytes {
                break 'pages oldest;
            }
            bytes += size;
            entries.push(entry);
            oldest = Some(event.id);
        }
        before = next;
        if before.is_none() {
            break None;
        }
    };
    entries.reverse();
    events::add_tool_names(&mut entries);
    Ok((entries, next_before))
}
