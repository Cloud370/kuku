use std::path::Path;

use serde_json::{json, Value};

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
                "type": { "type": "string", "enum": ["UserInput", "ModelResponse", "ToolCall", "ToolResult", "PermissionRequested", "PermissionAllow", "PermissionDeny", "ContextSkills", "Handoff", "TurnRollback", "TurnRollbackUndo"], "description": "Filter by event type" },
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
        filtered_refs = crate::context::revert::filter_rolled_back_events(&all_events);
        &filtered_refs
    } else {
        &[]
    };

    let type_filter = args
        .get("type")
        .and_then(Value::as_str)
        .map(normalize_type_filter);
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

        if type_filter.is_none() && !include_in_default_results(&event.payload) {
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

        if let Some(ref filter) = type_filter {
            if !event_type_matches(&event.payload, filter) {
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

fn build_turn_map_from_events(
    events: &[crate::event::StoredEvent],
) -> std::collections::HashMap<u64, usize> {
    let mut map = std::collections::HashMap::new();
    let mut current_turn: usize = 0;
    let mut seen_turn_end = false;

    for event in events.iter().rev() {
        map.insert(event.id, current_turn);
        if matches!(event.payload, EventPayload::TurnEnd { .. }) {
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
        if matches!(event.payload, EventPayload::TurnEnd { .. }) {
            if seen_turn_end {
                current_turn += 1;
            }
            seen_turn_end = true;
        }
    }

    map
}

fn normalize_type_filter(raw: &str) -> String {
    match raw {
        "UserInput" => "user.input".to_string(),
        "ModelResponse" => "model.response".to_string(),
        "ToolCall" => "tool.call".to_string(),
        "ToolResult" => "tool.result".to_string(),
        "PermissionRequested" => "permission.requested".to_string(),
        "PermissionAllow" => "permission.allow".to_string(),
        "PermissionDeny" => "permission.deny".to_string(),
        "ContextSkills" => "context.skills".to_string(),
        "Handoff" => "handoff".to_string(),
        "TurnRollback" => "turn.rollback".to_string(),
        "TurnRollbackUndo" => "turn.rollback.undo".to_string(),
        other => other.to_lowercase(),
    }
}

fn event_type_matches(payload: &EventPayload, filter: &str) -> bool {
    let type_tag = match payload {
        EventPayload::UserInput { .. } => "user.input",
        EventPayload::ModelResponse { .. } => "model.response",
        EventPayload::ToolCall { .. } => "tool.call",
        EventPayload::ToolResult { .. } => "tool.result",
        EventPayload::PermissionRequested { .. } => "permission.requested",
        EventPayload::PermissionAllow { .. } => "permission.allow",
        EventPayload::PermissionDeny { .. } => "permission.deny",
        EventPayload::ContextSkills { .. } => "context.skills",
        EventPayload::Handoff { .. } => "handoff",
        EventPayload::TurnRollback { .. } => "turn.rollback",
        EventPayload::TurnRollbackUndo { .. } => "turn.rollback.undo",
        EventPayload::SessionMeta { .. }
        | EventPayload::ContextPrelude { .. }
        | EventPayload::ContextSources { .. }
        | EventPayload::TurnStart { .. }
        | EventPayload::TurnEnd { .. }
        | EventPayload::ModelError { .. }
        | EventPayload::Unknown(_) => return false,
    };
    type_tag == filter
}

fn include_in_default_results(payload: &EventPayload) -> bool {
    !matches!(payload, EventPayload::ContextSkills { .. })
}

fn format_event(event: &crate::event::StoredEvent) -> String {
    let mut json = serde_json::json!({
        "id": event.id,
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
mod tests {
    use super::*;
    use crate::event::{EventPayload, EventStore};
    use serde_json::json;
    use tempfile::tempdir;

    fn write_events(dir: &Path, payloads: &[EventPayload]) -> std::path::PathBuf {
        let path = dir.join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        for payload in payloads {
            store.append(payload.clone()).unwrap();
        }
        path
    }

    fn ts(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn query_session_filters_by_type() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "hello".into(),
                },
                EventPayload::ModelResponse {
                    turn: 1,
                    ts: ts("t"),
                    request_id: "r1".into(),
                    text: "hi".into(),
                    thinking: None,
                    input_tokens_total: None,
                },
                EventPayload::TurnEnd {
                    turn: 1,
                    ts: ts("t"),
                },
            ],
        );
        let result = query_session(&json!({"type": "UserInput"}), &path);
        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("hello"));
        assert!(!result.model_content.contains("\"hi\""));
    }

    #[test]
    fn query_session_default_results_exclude_context_skills() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::ContextSkills {
                    turn: 1,
                    ts: ts("t"),
                    registry: crate::skill::registry::SkillRegistry::builder().build(),
                    bootstrap_loaded: vec!["bootstrap-alpha".into()],
                },
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "hello".into(),
                },
            ],
        );

        let default_result = query_session(&json!({}), &path);
        assert_eq!(default_result.status, "ok");
        assert!(default_result.model_content.contains("hello"));
        assert!(!default_result.model_content.contains("context.skills"));
        assert!(!default_result.model_content.contains("bootstrap-alpha"));

        let explicit_result = query_session(&json!({"type": "ContextSkills"}), &path);
        assert_eq!(explicit_result.status, "ok");
        assert!(explicit_result.model_content.contains("context.skills"));
        assert!(explicit_result.model_content.contains("bootstrap-alpha"));
    }

    #[test]
    fn query_session_text_search() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "build auth".into(),
                },
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "fix bug".into(),
                },
            ],
        );
        let result = query_session(&json!({"search": "auth"}), &path);
        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("build auth"));
        assert!(!result.model_content.contains("fix bug"));
    }

    #[test]
    fn query_session_respects_limit() {
        let dir = tempdir().unwrap();
        let mut payloads = Vec::new();
        for i in 0..30 {
            payloads.push(EventPayload::UserInput {
                turn: 1,
                ts: ts("t"),
                text: format!("msg {i}"),
            });
        }
        let path = write_events(dir.path(), &payloads);
        let result = query_session(&json!({"limit": 5}), &path);
        assert_eq!(result.status, "ok");
        let count = result.model_content.matches("\"id\":").count();
        assert_eq!(count, 5);
    }

    #[test]
    fn query_session_turn_filtering() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "first turn".into(),
                },
                EventPayload::TurnEnd {
                    turn: 1,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 2,
                    ts: ts("t"),
                    text: "second turn".into(),
                },
                EventPayload::TurnEnd {
                    turn: 2,
                    ts: ts("t"),
                },
            ],
        );
        let result = query_session(&json!({"from_turn": 0, "to_turn": 0}), &path);
        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("second turn"));
        assert!(!result.model_content.contains("first turn"));
    }

    #[test]
    fn query_session_empty_events() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let result = query_session(&json!({}), &path);
        assert_eq!(result.status, "ok");
        assert_eq!(result.model_content, "[]");
    }

    #[test]
    fn query_session_truncates_long_content() {
        let dir = tempdir().unwrap();
        let long_text = "x".repeat(1000);
        let path = write_events(
            dir.path(),
            &[EventPayload::UserInput {
                turn: 1,
                ts: ts("t"),
                text: long_text,
            }],
        );
        let result = query_session(&json!({}), &path);
        assert_eq!(result.status, "ok");
        // Content should be truncated: 1000 chars → 500 + "...(truncated)"
        assert!(result.model_content.contains("...(truncated)"));
        // The full 1000-char string should NOT appear
        let full = "x".repeat(1000);
        assert!(!result.model_content.contains(&full));
    }

    #[test]
    fn query_session_output_cap_drops_earliest_events() {
        let dir = tempdir().unwrap();
        let big_text = "y".repeat(3000);
        let mut payloads = Vec::new();
        for i in 0..5 {
            payloads.push(EventPayload::UserInput {
                turn: 1,
                ts: ts("t"),
                text: format!("msg_{i}_{big_text}"),
            });
        }
        let path = write_events(dir.path(), &payloads);
        let result = query_session(&json!({}), &path);
        assert_eq!(result.status, "ok");
        assert!(
            result.model_content.len() < MAX_RESULT_CHARS + 2000,
            "output too large: {} chars",
            result.model_content.len()
        );
    }

    #[test]
    fn query_session_skip_rolled_back_filters_events() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::TurnStart {
                    turn: 1,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "original".into(),
                },
                EventPayload::TurnEnd {
                    turn: 1,
                    ts: ts("t"),
                },
                EventPayload::TurnStart {
                    turn: 2,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 2,
                    ts: ts("t"),
                    text: "rolled back".into(),
                },
                EventPayload::TurnEnd {
                    turn: 2,
                    ts: ts("t"),
                },
                EventPayload::TurnRollback {
                    turn: 3,
                    ts: ts("t"),
                    target_turn: 2,
                    scope: crate::event::RollbackScope::ConversationOnly,
                },
            ],
        );
        let result = query_session(&json!({"skip_rolled_back": true}), &path);
        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("original"));
        assert!(!result.model_content.contains("rolled back"));
    }

    #[test]
    fn query_session_skip_rolled_back_false_returns_all() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::TurnStart {
                    turn: 1,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "original".into(),
                },
                EventPayload::TurnEnd {
                    turn: 1,
                    ts: ts("t"),
                },
                EventPayload::TurnStart {
                    turn: 2,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 2,
                    ts: ts("t"),
                    text: "rolled back".into(),
                },
                EventPayload::TurnEnd {
                    turn: 2,
                    ts: ts("t"),
                },
                EventPayload::TurnRollback {
                    turn: 3,
                    ts: ts("t"),
                    target_turn: 2,
                    scope: crate::event::RollbackScope::ConversationOnly,
                },
            ],
        );
        let result = query_session(&json!({"skip_rolled_back": false}), &path);
        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("original"));
        assert!(result.model_content.contains("rolled back"));
    }

    #[test]
    fn query_session_type_filter_turn_rollback() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "hello".into(),
                },
                EventPayload::TurnRollback {
                    turn: 2,
                    ts: ts("t"),
                    target_turn: 1,
                    scope: crate::event::RollbackScope::Both,
                },
            ],
        );
        let result = query_session(
            &json!({"type": "TurnRollback", "skip_rolled_back": false}),
            &path,
        );
        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("turn.rollback"));
        assert!(!result.model_content.contains("hello"));
    }

    #[test]
    fn query_session_type_filter_permission_requested() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "hello".into(),
                },
                EventPayload::PermissionRequested {
                    turn: 1,
                    ts: ts("t"),
                    tool_call_id: "toolu_cmd".into(),
                    tool: "run_command".into(),
                    risk: "command".into(),
                    summary: "run tests".into(),
                    candidate: "cargo test".into(),
                    source: "default_ask".into(),
                },
            ],
        );

        let result = query_session(&json!({"type": "PermissionRequested"}), &path);

        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("permission.requested"));
        assert!(result.model_content.contains("cargo test"));
        assert!(!result.model_content.contains("hello"));
    }

    #[test]
    fn query_session_type_filter_permission_allow_and_deny() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "hello".into(),
                },
                EventPayload::PermissionAllow {
                    turn: 1,
                    ts: ts("t"),
                    tool_call_id: "toolu_allow".into(),
                    tool: "run_command".into(),
                    scope: "session".into(),
                    matcher: "cargo test".into(),
                    source: "user".into(),
                },
                EventPayload::PermissionDeny {
                    turn: 1,
                    ts: ts("t"),
                    tool_call_id: "toolu_deny".into(),
                    tool: "run_command".into(),
                    reason: "too risky".into(),
                    source: "user".into(),
                },
            ],
        );

        let allow = query_session(&json!({"type": "permission.allow"}), &path);
        let deny = query_session(&json!({"type": "permission.deny"}), &path);

        assert_eq!(allow.status, "ok");
        assert!(allow.model_content.contains("permission.allow"));
        assert!(allow.model_content.contains("cargo test"));
        assert!(!allow.model_content.contains("permission.deny"));
        assert!(!allow.model_content.contains("hello"));
        assert_eq!(deny.status, "ok");
        assert!(deny.model_content.contains("permission.deny"));
        assert!(deny.model_content.contains("too risky"));
        assert!(!deny.model_content.contains("permission.allow"));
        assert!(!deny.model_content.contains("hello"));
    }

    #[test]
    fn query_session_turn_filtering_respects_skip_rolled_back_stream() {
        let dir = tempdir().unwrap();
        let path = write_events(
            dir.path(),
            &[
                EventPayload::TurnStart {
                    turn: 1,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 1,
                    ts: ts("t"),
                    text: "first".into(),
                },
                EventPayload::TurnEnd {
                    turn: 1,
                    ts: ts("t"),
                },
                EventPayload::TurnStart {
                    turn: 2,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 2,
                    ts: ts("t"),
                    text: "second active".into(),
                },
                EventPayload::TurnEnd {
                    turn: 2,
                    ts: ts("t"),
                },
                EventPayload::TurnStart {
                    turn: 3,
                    ts: ts("t"),
                },
                EventPayload::UserInput {
                    turn: 3,
                    ts: ts("t"),
                    text: "third rolled back".into(),
                },
                EventPayload::TurnEnd {
                    turn: 3,
                    ts: ts("t"),
                },
                EventPayload::TurnRollback {
                    turn: 4,
                    ts: ts("t"),
                    target_turn: 3,
                    scope: crate::event::RollbackScope::ConversationOnly,
                },
            ],
        );

        let result = query_session(
            &json!({"skip_rolled_back": true, "from_turn": 1, "to_turn": 1}),
            &path,
        );

        assert_eq!(result.status, "ok");
        assert!(result.model_content.contains("first"));
        assert!(!result.model_content.contains("second active"));
        assert!(!result.model_content.contains("third rolled back"));
    }
}
