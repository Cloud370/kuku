use std::path::Path;

use serde_json::{json, Value};

use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, EventStore};
use crate::tool::ToolResultEnvelope;

const MAX_RESULT_CHARS: usize = 8_000;
const MAX_EVENT_CONTENT_CHARS: usize = 500;
const DEFAULT_LIMIT: usize = 20;

pub(crate) fn query_session_definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: "query_session".to_string(),
        description: "Query historical session events that are no longer in your visible conversation context. DO NOT use this tool if the information you need is already present in the messages above. Only call when you need to recall details from earlier in the session that have been handed off or are otherwise outside your current context window.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "search": { "type": "string", "description": "Text to search for in event content" },
                "kind": { "type": "string", "description": "Filter by event kind" },
                "conversation": { "type": "string", "description": "Filter by conversation address" },
                "after": { "type": "integer", "description": "Return events with id greater than this value" },
                "from_turn": { "type": "integer", "description": "Start from N turns ago (0 = most recent)" },
                "to_turn": { "type": "integer", "description": "Up to N turns ago (inclusive)" },
                "limit": { "type": "integer", "description": "Max events to return (default 20)" },
                "skip_rolled_back": { "type": "boolean", "description": "Skip events in rolled-back turns (default: true)", "default": true }
            }
        }),
        read_only: true,
        max_result_chars: MAX_RESULT_CHARS,
        risk: "read".to_string(),
    }
}

pub(crate) fn query_session(args: &Value, events_path: &Path) -> ToolResultEnvelope {
    let (content, count) = match run_query(args, events_path) {
        Ok(pair) => pair,
        Err(e) => {
            return ToolResultEnvelope::error(
                format!("query_session failed: {e}"),
                format!("error querying session: {e}"),
            );
        }
    };
    ToolResultEnvelope {
        status: "ok".to_string(),
        summary: format!("{count} events returned"),
        model_content: content,
        truncated: false,
        structured: None,
    }
}

fn run_query(args: &Value, events_path: &Path) -> Result<(String, usize), crate::error::Error> {
    let all_events = EventStore::replay(events_path)?;
    if all_events.is_empty() {
        return Ok(("[]".to_string(), 0));
    }

    let skip_rolled_back = args
        .get("skip_rolled_back")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let filtered_refs;
    let events: &[&crate::event::StoredEvent] = if skip_rolled_back {
        filtered_refs = queryable_filtered_events(&all_events);
        &filtered_refs
    } else {
        &[]
    };

    let kind_filter = args.get("kind").and_then(Value::as_str);
    let conversation_filter = args.get("conversation").and_then(Value::as_str);
    let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
    let search = args.get("search").and_then(Value::as_str);
    let from_turn = args.get("from_turn").and_then(Value::as_u64).unwrap_or(0) as usize;
    let to_turn = args
        .get("to_turn")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_LIMIT as u64) as usize;

    let turn_map = if skip_rolled_back {
        build_turn_map_from_filtered(events)
    } else {
        build_turn_map_from_events(&all_events)
    };
    let turn_conversations = build_turn_conversations(&all_events);

    let iter: Box<dyn Iterator<Item = &crate::event::StoredEvent>> = if skip_rolled_back {
        Box::new(events.iter().rev().copied())
    } else {
        Box::new(all_events.iter().rev())
    };

    let mut matched = Vec::new();
    for event in iter {
        if matched.len() >= limit {
            break;
        }

        if event.id <= after {
            continue;
        }

        if kind_filter.is_none() && !include_in_default_results(&event.payload) {
            continue;
        }

        let Some(&turn_idx) = turn_map.get(&event.id) else {
            continue;
        };
        if turn_idx < from_turn {
            continue;
        }
        if let Some(to) = to_turn {
            if turn_idx > to {
                continue;
            }
        }

        if let Some(filter) = kind_filter {
            if !event_type_matches(&event.payload, filter) {
                continue;
            }
        }

        if let Some(conversation) = conversation_filter {
            if !event_matches_conversation(&event.payload, conversation, &turn_conversations) {
                continue;
            }
        }

        if let Some(query) = search {
            let serialized = serde_json::to_string(&event.payload).unwrap_or_default();
            if !serialized.to_lowercase().contains(&query.to_lowercase()) {
                continue;
            }
        }

        matched.push(event);
    }

    matched.reverse();

    let mut output = String::from("[\n");
    let mut total_chars = 2;
    for (i, event) in matched.iter().enumerate() {
        let entry = format_event(event);
        let entry_chars = entry.chars().count();
        if total_chars + entry_chars + 4 > MAX_RESULT_CHARS && i > 0 {
            output.push_str("\n... (truncated, total output cap reached)");
            break;
        }
        if i > 0 {
            output.push_str(",\n");
        }
        output.push_str(&entry);
        total_chars += entry_chars + 2;
    }
    output.push_str("\n]");
    Ok((output, matched.len()))
}

fn queryable_filtered_events(
    all_events: &[crate::event::StoredEvent],
) -> Vec<&crate::event::StoredEvent> {
    let target_turn = active_main_turn_rollback_target(all_events);
    crate::context::revert::filter_rolled_back_events(all_events)
        .into_iter()
        .filter(|event| {
            let Some(target_turn) = target_turn else {
                return true;
            };
            match &event.payload {
                EventPayload::TurnStarted { turn, .. }
                | EventPayload::MessageUser { turn, .. }
                | EventPayload::ModelResponse { turn, .. }
                | EventPayload::ModelError { turn, .. }
                | EventPayload::ToolCall {
                    turn,
                    conversation: None,
                    ..
                }
                | EventPayload::ToolResult {
                    turn,
                    conversation: None,
                    ..
                }
                | EventPayload::PermissionAllow { turn, .. }
                | EventPayload::PermissionRequested { turn, .. }
                | EventPayload::PermissionDeny { turn, .. }
                | EventPayload::ContextSources { turn, .. }
                | EventPayload::Handoff { turn, .. }
                | EventPayload::TurnCompleted { turn, .. }
                | EventPayload::TurnCancelled { turn, .. }
                | EventPayload::TurnInterrupted { turn, .. } => *turn < target_turn,
                _ => true,
            }
        })
        .collect()
}

fn active_main_turn_rollback_target(all_events: &[crate::event::StoredEvent]) -> Option<u64> {
    let undone = all_events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ConversationRollbackUndone {
                conversation,
                rollback_event_id,
                ..
            } if conversation == "main" => Some(*rollback_event_id),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();

    all_events
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::ConversationRollback {
                conversation,
                to_turn,
                scope,
                ..
            } if conversation == "main"
                && scope.affects_conversation()
                && !undone.contains(&event.id) =>
            {
                Some(*to_turn)
            }
            _ => None,
        })
}

fn build_turn_map_from_events(
    events: &[crate::event::StoredEvent],
) -> std::collections::HashMap<u64, usize> {
    let mut map = std::collections::HashMap::new();
    let mut current_turn: usize = 0;
    let mut seen_turn_end = false;

    for event in events.iter().rev() {
        map.insert(event.id, current_turn);
        if matches!(
            event.payload,
            EventPayload::TurnCompleted { .. }
                | EventPayload::TurnCancelled { .. }
                | EventPayload::TurnInterrupted { .. }
        ) {
            if seen_turn_end {
                current_turn += 1;
            }
            seen_turn_end = true;
        }
    }
    map
}

fn build_turn_map_from_filtered(
    events: &[&crate::event::StoredEvent],
) -> std::collections::HashMap<u64, usize> {
    let mut map = std::collections::HashMap::new();
    let mut current_turn: usize = 0;
    let mut seen_turn_end = false;

    for event in events.iter().rev() {
        map.insert(event.id, current_turn);
        if matches!(
            event.payload,
            EventPayload::TurnCompleted { .. }
                | EventPayload::TurnCancelled { .. }
                | EventPayload::TurnInterrupted { .. }
        ) {
            if seen_turn_end {
                current_turn += 1;
            }
            seen_turn_end = true;
        }
    }

    map
}

fn event_type_matches(payload: &EventPayload, filter: &str) -> bool {
    payload.kind_name() == filter
}

fn build_turn_conversations(
    events: &[crate::event::StoredEvent],
) -> std::collections::HashMap<u64, String> {
    let mut map = std::collections::HashMap::new();
    for event in events {
        if let (Some(turn), Some(conversation)) = (
            event_turn(&event.payload),
            event_conversation(&event.payload),
        ) {
            map.entry(turn).or_insert_with(|| conversation.to_string());
        }
    }
    map
}

fn event_matches_conversation(
    payload: &EventPayload,
    conversation: &str,
    turn_conversations: &std::collections::HashMap<u64, String>,
) -> bool {
    if let Some(value) = event_conversation(payload) {
        return value == conversation;
    }
    if let Some(turn) = event_turn(payload) {
        if let Some(value) = turn_conversations.get(&turn) {
            return value == conversation;
        }
    }
    conversation == ConversationAddress::MAIN.as_str() && unscoped_main_event(payload)
}

fn event_turn(payload: &EventPayload) -> Option<u64> {
    match payload {
        EventPayload::ContextSources { turn, .. }
        | EventPayload::ContextSkills { turn, .. }
        | EventPayload::ModelResponse { turn, .. }
        | EventPayload::ModelError { turn, .. }
        | EventPayload::ToolCall { turn, .. }
        | EventPayload::PermissionRequested { turn, .. }
        | EventPayload::PermissionAllow { turn, .. }
        | EventPayload::PermissionDeny { turn, .. }
        | EventPayload::ToolResult { turn, .. }
        | EventPayload::Handoff { turn, .. }
        | EventPayload::PromptSnapshot { turn, .. }
        | EventPayload::MessageUser { turn, .. }
        | EventPayload::MessageAssistant { turn, .. }
        | EventPayload::TurnStarted { turn, .. }
        | EventPayload::TurnCompleted { turn, .. }
        | EventPayload::TurnCancelled { turn, .. }
        | EventPayload::TurnInterrupted { turn, .. } => Some(*turn),
        EventPayload::SessionCreated { .. }
        | EventPayload::ConversationOpened { .. }
        | EventPayload::ConversationBound { .. }
        | EventPayload::ConversationRollback { .. }
        | EventPayload::ConversationRollbackUndone { .. }
        | EventPayload::Unknown(_) => None,
    }
}

fn unscoped_main_event(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::SessionCreated { .. }
            | EventPayload::ModelResponse { .. }
            | EventPayload::ModelError { .. }
            | EventPayload::ToolCall {
                conversation: None,
                ..
            }
            | EventPayload::ToolResult {
                conversation: None,
                ..
            }
            | EventPayload::PermissionRequested { .. }
            | EventPayload::PermissionAllow { .. }
            | EventPayload::PermissionDeny { .. }
            | EventPayload::ContextSources { .. }
            | EventPayload::Handoff { .. }
    )
}

fn include_in_default_results(payload: &EventPayload) -> bool {
    !matches!(payload, EventPayload::ContextSkills { .. })
}

fn format_event(event: &crate::event::StoredEvent) -> String {
    let mut json = serde_json::json!({
        "id": event.id,
        "kind": event.payload.kind_name(),
    });
    if let Ok(payload_json) = serde_json::to_value(&event.payload) {
        if let Some(obj) = payload_json.as_object() {
            for (k, v) in obj {
                let display_value = truncate_value(k, v);
                json[k] = display_value;
            }
        }
    }
    truncate_json_string(&serde_json::to_string(&json).unwrap_or_default())
}

fn event_conversation(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::ToolCall { conversation, .. }
        | EventPayload::ToolResult { conversation, .. } => conversation.as_deref(),
        EventPayload::ConversationOpened { conversation, .. }
        | EventPayload::ConversationBound { conversation, .. }
        | EventPayload::PromptSnapshot { conversation, .. }
        | EventPayload::MessageUser { conversation, .. }
        | EventPayload::MessageAssistant { conversation, .. }
        | EventPayload::TurnStarted { conversation, .. }
        | EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. }
        | EventPayload::ConversationRollback { conversation, .. }
        | EventPayload::ConversationRollbackUndone { conversation, .. }
        | EventPayload::ContextSkills { conversation, .. } => Some(conversation.as_str()),
        _ => None,
    }
}

fn truncate_value(key: &str, value: &Value) -> Value {
    match key {
        "text" | "model_content" | "summary" | "content" => {
            if let Some(s) = value.as_str() {
                if s.chars().count() > MAX_EVENT_CONTENT_CHARS {
                    Value::String(format!(
                        "{}...(truncated)",
                        s.chars().take(MAX_EVENT_CONTENT_CHARS).collect::<String>()
                    ))
                } else {
                    value.clone()
                }
            } else {
                value.clone()
            }
        }
        _ => value.clone(),
    }
}

fn truncate_json_string(s: &str) -> String {
    if s.chars().count() <= MAX_EVENT_CONTENT_CHARS + 100 {
        s.to_string()
    } else {
        format!(
            "{}...(truncated)",
            s.chars()
                .take(MAX_EVENT_CONTENT_CHARS + 100)
                .collect::<String>()
        )
    }
}

#[cfg(test)]
#[path = "query_session/tests.rs"]
mod tests;
