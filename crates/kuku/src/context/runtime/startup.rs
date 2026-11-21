use std::collections::BTreeMap;
use std::path::Path;

use crate::event::{EventPayload, StoredEvent};
use crate::{Error, Result};

use super::{belongs, scan, SnapshotCache, MAX_SELECTION_BYTES_FACTOR};

pub(crate) fn read_agent_events(
    path: &Path,
    agent: &str,
    max_bytes: usize,
) -> Result<Vec<StoredEvent>> {
    let mut latest_turn = None;
    let mut metadata = BTreeMap::<String, StoredEvent>::new();
    let mut current = Vec::<StoredEvent>::new();
    let mut skills = Vec::<StoredEvent>::new();
    let mut snapshots = SnapshotCache::default();
    scan(path, 0, |mut event| {
        let mut changed = false;
        let key = match &event.payload {
            EventPayload::SessionCreated { .. } => Some("session".into()),
            EventPayload::ConversationOpened { conversation, .. } if conversation == agent => {
                Some("opened".into())
            }
            EventPayload::ConversationBound { conversation, .. } if conversation == agent => {
                Some("bound".into())
            }
            EventPayload::PromptSnapshot { conversation, .. } if conversation == agent => {
                Some("prompt".into())
            }
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["address"] == agent
                    && matches!(
                        value["record"]["operation"].as_str(),
                        Some("agent_created" | "agent_configured")
                    ) =>
            {
                value["record"]["operation"].as_str().map(str::to_string)
            }
            _ => None,
        };
        if let Some(key) = key {
            metadata.insert(key, event);
            return Ok(());
        }
        if let EventPayload::ContextSkills { conversation, .. } = &event.payload {
            if conversation == agent {
                if skills.len() == 2 {
                    skills.remove(0);
                }
                skills.push(event);
            }
            return Ok(());
        }
        if let EventPayload::TurnStarted {
            turn, conversation, ..
        } = &event.payload
        {
            if conversation == agent {
                latest_turn = Some(*turn);
                current.clear();
                current.push(event);
            }
            return Ok(());
        }
        if let EventPayload::Unknown(value) = &event.payload {
            if value["agent"] == agent {
                snapshots.observe_context(value);
            }
        }
        changed |= snapshots.observe_result(&mut event, agent, latest_turn);
        let turn = match &event.payload {
            EventPayload::ModelResponse {
                turn, conversation, ..
            }
            | EventPayload::ModelError {
                turn, conversation, ..
            }
            | EventPayload::ToolCall {
                turn, conversation, ..
            }
            | EventPayload::ToolResult {
                turn, conversation, ..
            } if belongs(conversation.as_deref(), *turn, agent, latest_turn) => Some(*turn),
            EventPayload::PermissionRequested { turn, .. }
            | EventPayload::PermissionAllow { turn, .. }
            | EventPayload::PermissionDeny { turn, .. } => Some(*turn),
            EventPayload::ModelRecovery {
                turn, conversation, ..
            } if conversation == agent => Some(*turn),
            EventPayload::TurnCompleted {
                turn, conversation, ..
            }
            | EventPayload::TurnCancelled {
                turn, conversation, ..
            }
            | EventPayload::TurnInterrupted {
                turn, conversation, ..
            }
            | EventPayload::MessageUser {
                turn, conversation, ..
            } if conversation == agent => Some(*turn),
            EventPayload::Unknown(value)
                if value["agent"] == agent
                    && matches!(
                        value["kind"].as_str(),
                        Some("runtime.tool_prepared" | "runtime.tool_dispatched")
                    ) =>
            {
                value["turn"].as_u64()
            }
            EventPayload::Unknown(value)
                if value["agent"] == agent
                    && value["kind"] == "runtime.model_retry"
                    && value["recovery"] == "regenerate_tool_arguments" =>
            {
                value["turn"].as_u64()
            }
            _ => None,
        };
        if turn.is_some() && turn == latest_turn {
            match &mut event.payload {
                EventPayload::ModelResponse {
                    conversation,
                    text,
                    thinking,
                    ..
                } => {
                    *conversation = Some(agent.into());
                    text.clear();
                    *thinking = None;
                }
                EventPayload::ModelError {
                    conversation,
                    message,
                    ..
                } => {
                    *conversation = Some(agent.into());
                    message.clear();
                }
                EventPayload::ToolCall { conversation, .. }
                | EventPayload::ToolResult { conversation, .. } => {
                    *conversation = Some(agent.into());
                }
                _ => {}
            }
            if let EventPayload::ToolResult {
                model_content,
                summary,
                structured,
                files_read,
                files_changed,
                commands_run,
                memory_changed,
                ..
            } = &mut event.payload
            {
                model_content.clear();
                summary.clear();
                *structured = None;
                files_read.clear();
                files_changed.clear();
                commands_run.clear();
                *memory_changed = None;
            }
            current.push(event);
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        trim_snapshot_cache(&metadata, &current, &skills, &mut snapshots, max_bytes)?;
        Ok(())
    })?;
    trim_snapshot_cache(&metadata, &current, &skills, &mut snapshots, max_bytes)?;
    let selected = metadata
        .into_values()
        .chain(current)
        .chain(skills)
        .chain(snapshots.into_events())
        .map(|event| (event.id, event))
        .collect::<BTreeMap<_, _>>();
    let result = selected.into_values().collect::<Vec<_>>();
    if serde_json::to_vec(&result)?.len() > max_bytes.saturating_mul(MAX_SELECTION_BYTES_FACTOR) {
        return Err(Error::InvalidArgument(
            "agent startup facts exceed the context selection byte limit".into(),
        ));
    }
    Ok(result)
}

fn trim_snapshot_cache(
    metadata: &BTreeMap<String, StoredEvent>,
    current: &[StoredEvent],
    skills: &[StoredEvent],
    snapshots: &mut SnapshotCache,
    max_bytes: usize,
) -> Result<()> {
    let budget = max_bytes.saturating_mul(MAX_SELECTION_BYTES_FACTOR);
    while serde_json::to_vec(&(
        metadata,
        current,
        skills,
        snapshots.entries.values().collect::<Vec<_>>(),
    ))?
    .len()
        > budget
    {
        if !snapshots.remove_oldest() {
            return Err(Error::InvalidArgument(
                "agent startup facts exceed the context selection byte limit".into(),
            ));
        }
    }
    Ok(())
}
