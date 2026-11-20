use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use kuku::event::{EventPayload, EventStore, StoredEvent};
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

#[derive(Clone)]
struct Operation {
    rewind_event_id: u64,
    target_event_id: u64,
    agent: String,
    turn_number: Option<u64>,
}

pub(super) struct RewindIndex {
    groups_by_target: HashMap<u64, Vec<RewindGroup>>,
    groups_by_id: HashMap<u64, RewindGroup>,
    members_by_group: HashMap<u64, Vec<u64>>,
}

impl RewindIndex {
    pub(super) fn new(events: &[StoredEvent], turns: &TurnLookup) -> Self {
        let owners = kuku::context::revert::event_owners(events);
        let events_by_id = events
            .iter()
            .map(|event| (event.id, event))
            .collect::<HashMap<_, _>>();
        let operations = operations(events, &owners, turns, &events_by_id);
        let mut by_agent: HashMap<&str, Vec<&Operation>> = HashMap::new();
        for operation in &operations {
            by_agent
                .entry(&operation.agent)
                .or_default()
                .push(operation);
        }

        let mut members_by_group: HashMap<u64, Vec<u64>> = HashMap::new();
        for event in events {
            let Some(agent) = owners.get(&event.id) else {
                continue;
            };
            let Some(operation) = by_agent.get(agent.as_str()).and_then(|operations| {
                operations.iter().find(|operation| {
                    event.id < operation.rewind_event_id && operation.target_event_id <= event.id
                })
            }) else {
                continue;
            };
            if history_entry_for_agent(event, turns, agent).is_some() {
                members_by_group
                    .entry(operation.rewind_event_id)
                    .or_default()
                    .push(event.id);
            }
        }

        let mut groups_by_target: HashMap<u64, Vec<RewindGroup>> = HashMap::new();
        let mut groups_by_id = HashMap::new();
        for operation in operations {
            let entry_count = members_by_group
                .get(&operation.rewind_event_id)
                .map_or(0, Vec::len);
            let group = RewindGroup {
                rewind_event_id: operation.rewind_event_id,
                target_event_id: operation.target_event_id,
                agent: operation.agent,
                turn_number: operation.turn_number,
                entry_count,
            };
            groups_by_target
                .entry(group.target_event_id)
                .or_default()
                .push(group.clone());
            groups_by_id.insert(group.rewind_event_id, group);
        }
        Self {
            groups_by_target,
            groups_by_id,
            members_by_group,
        }
    }

    pub(super) fn groups_for_agent(&self, agent: &str) -> HashMap<u64, Vec<RewindGroup>> {
        self.groups_by_target
            .iter()
            .filter_map(|(target, groups)| {
                let groups = groups
                    .iter()
                    .filter(|group| group.agent == agent)
                    .cloned()
                    .collect::<Vec<_>>();
                let mut groups = groups;
                groups.sort_by_key(|group| group.rewind_event_id);
                (!groups.is_empty()).then_some((*target, groups))
            })
            .collect()
    }

    fn group(&self, rewind_event_id: u64, agent: &str) -> Option<&RewindGroup> {
        self.groups_by_id
            .get(&rewind_event_id)
            .filter(|group| group.agent == agent)
    }

    fn members(&self, rewind_event_id: u64) -> Option<&[u64]> {
        self.members_by_group
            .get(&rewind_event_id)
            .map(Vec::as_slice)
    }
}

fn operations(
    events: &[StoredEvent],
    owners: &HashMap<u64, String>,
    turns: &TurnLookup,
    events_by_id: &HashMap<u64, &StoredEvent>,
) -> Vec<Operation> {
    events
        .iter()
        .filter_map(|event| {
            let EventPayload::Unknown(value) = &event.payload else {
                return None;
            };
            if value["kind"] != "runtime.operation"
                || value["record"]["operation"] != "message_rewind"
            {
                return None;
            }
            let record = &value["record"];
            let agent = record["address"].as_str()?;
            let target_event_id = record["target_event_id"].as_u64()?;
            if target_event_id >= event.id
                || owners.get(&target_event_id).map(String::as_str) != Some(agent)
            {
                return None;
            }
            let target = events_by_id.get(&target_event_id)?;
            let EventPayload::MessageUser { turn, .. } = &target.payload else {
                return None;
            };
            Some(Operation {
                rewind_event_id: event.id,
                target_event_id,
                agent: agent.to_owned(),
                turn_number: turns.number(agent, *turn),
            })
        })
        .collect()
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
    let journal = EventStore::replay(&target.path)?;
    let turns = manager(&state).await.turn_lookup(&target.path, head)?;
    let index = RewindIndex::new(&journal, &turns);
    let group = index.group(rewind_event_id, &query.agent).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "rewind_group_not_found",
            "rewind group not found for this Agent",
        )
    })?;
    let before = query
        .before
        .unwrap_or(group.rewind_event_id)
        .min(group.rewind_event_id);
    let members = index.members(rewind_event_id).unwrap_or_default();
    let events_by_id = journal
        .iter()
        .map(|event| (event.id, event))
        .collect::<HashMap<_, _>>();
    let (entries, next_before) =
        group_page(&events_by_id, members, before, limit, &turns, &query.agent)?;
    let response = json!({"ok":true,"entries":entries,"next_before":next_before});
    events::validate_response_bytes(&response)?;
    Ok(Json(response))
}

fn group_page(
    events: &HashMap<u64, &StoredEvent>,
    members: &[u64],
    before: u64,
    limit: usize,
    turns: &TurnLookup,
    agent: &str,
) -> Result<(Vec<Value>, Option<u64>)> {
    let mut entries = Vec::new();
    let mut bytes = 0;
    let max_item_bytes =
        events::HISTORY_PAGE_BYTES.saturating_sub(events::HISTORY_RESPONSE_OVERHEAD);
    let end = members.partition_point(|id| *id < before);
    let candidates = &members[..end];
    let mut oldest = None;
    for id in candidates.iter().rev() {
        let Some(event) = events.get(id) else {
            continue;
        };
        let Some(entry) = history_entry_for_agent(event, turns, agent) else {
            continue;
        };
        let size = serde_json::to_vec(&entry).map_err(kuku::Error::from)?.len();
        if size > max_item_bytes {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "history_entry_too_large",
                "history item exceeds the response byte limit",
            ));
        }
        if !entries.is_empty() && bytes + size > max_item_bytes {
            break;
        }
        bytes += size;
        entries.push(entry);
        oldest = Some(*id);
        if entries.len() == limit {
            break;
        }
    }
    entries.reverse();
    events::add_tool_names(&mut entries);
    let next_before = oldest.filter(|oldest| candidates.iter().any(|id| id < oldest));
    Ok((entries, next_before))
}
