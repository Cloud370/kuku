use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

#[derive(Deserialize)]
pub struct EventsQuery {
    pub after: Option<u64>,
    pub conversation: Option<String>,
    pub workspace: Option<String>,
}

fn event_conversation(payload: &kuku::event::EventPayload) -> Option<&str> {
    match payload {
        kuku::event::EventPayload::ToolCall { conversation, .. }
        | kuku::event::EventPayload::ToolResult { conversation, .. } => conversation.as_deref(),
        kuku::event::EventPayload::ConversationOpened { conversation, .. }
        | kuku::event::EventPayload::ConversationBound { conversation, .. }
        | kuku::event::EventPayload::PromptSnapshot { conversation, .. }
        | kuku::event::EventPayload::MessageUser { conversation, .. }
        | kuku::event::EventPayload::MessageAssistant { conversation, .. }
        | kuku::event::EventPayload::TurnStarted { conversation, .. }
        | kuku::event::EventPayload::TurnCompleted { conversation, .. }
        | kuku::event::EventPayload::TurnCancelled { conversation, .. }
        | kuku::event::EventPayload::TurnInterrupted { conversation, .. }
        | kuku::event::EventPayload::ConversationRollback { conversation, .. }
        | kuku::event::EventPayload::ConversationRollbackUndone { conversation, .. }
        | kuku::event::EventPayload::ContextSkills { conversation, .. } => Some(conversation),
        kuku::event::EventPayload::Unknown(value) => {
            value.get("conversation").and_then(|item| item.as_str())
        }
        _ => None,
    }
}

fn event_matches_conversation(payload: &kuku::event::EventPayload, conversation: &str) -> bool {
    match event_conversation(payload) {
        Some(value) => value == conversation,
        None => {
            matches!(payload, kuku::event::EventPayload::SessionCreated { .. })
                || conversation == "main"
        }
    }
}

fn stream_event_conversation(value: &serde_json::Value) -> Option<&str> {
    value
        .get("conversation")
        .and_then(|item| item.as_str())
        .or_else(|| {
            value
                .get("kind")
                .and_then(|kind| kind.get("agent"))
                .and_then(|agent| agent.get("conversation"))
                .and_then(|item| item.as_str())
        })
}

fn stream_event_agent_conversation(value: &serde_json::Value) -> Option<&str> {
    if value.get("type").and_then(|item| item.as_str()) != Some("tool_start") {
        return None;
    }

    value
        .get("kind")
        .and_then(|kind| kind.get("agent"))
        .and_then(|agent| agent.get("conversation"))
        .and_then(|item| item.as_str())
}

struct StreamConversationFilter<'a> {
    conversation: &'a str,
    agent_tool_conversations: HashMap<String, String>,
}

impl<'a> StreamConversationFilter<'a> {
    fn new(conversation: &'a str) -> Self {
        Self {
            conversation,
            agent_tool_conversations: HashMap::new(),
        }
    }

    fn matches(&mut self, value: &serde_json::Value) -> bool {
        let event_type = value.get("type").and_then(|item| item.as_str());
        let id = value.get("id").and_then(|item| item.as_str());

        if let (Some("tool_start"), Some(id), Some(conversation)) =
            (event_type, id, stream_event_agent_conversation(value))
        {
            self.agent_tool_conversations
                .insert(id.to_string(), conversation.to_string());
        }

        let matched_tool_conversation = id.and_then(|id| self.agent_tool_conversations.get(id));
        let matches = if let Some(conversation) = matched_tool_conversation {
            conversation == self.conversation
        } else if let Some(conversation) = stream_event_conversation(value) {
            conversation == self.conversation
        } else {
            self.conversation == "main"
        };

        if event_type == Some("tool_end") {
            if let Some(id) = id {
                self.agent_tool_conversations.remove(id);
            }
        }

        matches
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::StreamConversationFilter;

    fn filter_stream_events(
        values: Vec<serde_json::Value>,
        conversation: &str,
    ) -> Vec<serde_json::Value> {
        let mut filter = StreamConversationFilter::new(conversation);
        values
            .into_iter()
            .filter(|value| filter.matches(value))
            .collect()
    }

    #[test]
    fn stream_conversation_filter_rejects_unscoped_live_events() {
        let event = json!({"event": "line", "text": "unscoped output"});

        let mut filter = StreamConversationFilter::new("review");

        assert!(!filter.matches(&event));
    }

    #[test]
    fn stream_conversation_filter_keeps_delegated_agent_tool_chain() {
        let events = filter_stream_events(
            vec![
                json!({"type": "text", "content": "main text"}),
                json!({"type": "tool_start", "id": "agent_review", "tool": "agent", "kind": {"agent": {"conversation": "review", "binding_id": null}}, "conversation": "review"}),
                json!({"type": "tool_output", "id": "agent_review", "event": {"text": "review child text"}}),
                json!({"type": "tool_output", "id": "agent_review", "event": {"tool_start": {"id": "child_tool", "tool": "read_file", "summary": "read", "kind": "simple"}}}),
                json!({"type": "tool_output", "id": "other_agent", "event": {"text": "other child text"}}),
                json!({"type": "tool_end", "id": "agent_review", "status": "ok", "summary": "done"}),
                json!({"type": "done", "conversation": "review", "text": "review done"}),
            ],
            "review",
        );

        assert_eq!(events.len(), 5);
        assert!(events.iter().any(|event| event["type"] == "tool_start"));
        assert!(events
            .iter()
            .any(|event| event["event"]["text"] == "review child text"));
        assert!(events
            .iter()
            .any(|event| event["event"]["tool_start"]["id"] == "child_tool"));
        assert!(events.iter().any(|event| event["type"] == "tool_end"));
        assert!(events.iter().any(|event| event["type"] == "done"));
        assert!(!events.iter().any(|event| event["content"] == "main text"));
        assert!(!events
            .iter()
            .any(|event| event["event"]["text"] == "other child text"));
    }

    #[test]
    fn main_stream_filter_keeps_unscoped_main_events_without_delegated_child_output() {
        let events = filter_stream_events(
            vec![
                json!({"type": "text", "content": "main text"}),
                json!({"type": "tool_start", "id": "agent_review", "tool": "agent", "kind": {"agent": {"conversation": "review", "binding_id": null}}, "conversation": "review"}),
                json!({"type": "tool_output", "id": "agent_review", "event": {"text": "review child text"}}),
                json!({"type": "tool_end", "id": "agent_review", "status": "ok", "summary": "done"}),
                json!({"type": "done", "conversation": "main", "text": "main done"}),
            ],
            "main",
        );

        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|event| event["content"] == "main text"));
        assert!(events.iter().any(|event| event["type"] == "done"));
        assert!(!events.iter().any(|event| event["id"] == "agent_review"));
        assert!(!events
            .iter()
            .any(|event| event["event"]["text"] == "review child text"));
    }
}

pub async fn events(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<EventsQuery>,
) -> Json<serde_json::Value> {
    let home = &state.kuku_home;

    let workspace = match params.workspace {
        Some(ws) => std::path::PathBuf::from(ws),
        None => match kuku::session::current_workspace() {
            Ok(w) => w,
            Err(_) => {
                return Json(
                    json!({"ok": false, "code": "invalid_request", "message": "workspace parameter required"}),
                );
            }
        },
    };

    let events_path = match kuku::session::session_events_path(home, &workspace, &session_id) {
        Ok(p) => p,
        Err(_) => {
            return Json(
                json!({"ok": false, "code": "session_not_found", "message": "session not found"}),
            );
        }
    };

    if !events_path.exists() {
        return Json(
            json!({"ok": false, "code": "session_not_found", "message": "session not found"}),
        );
    }

    let events = match kuku::event::EventStore::replay(&events_path) {
        Ok(e) => e,
        Err(e) => return Json(json!({"ok": false, "code": "internal", "message": e.to_string()})),
    };

    let after = params.after.unwrap_or(0);
    let conversation = params.conversation.as_deref();
    let filtered: Vec<_> = events
        .iter()
        .filter(|e| e.id > after)
        .filter(|event| {
            conversation
                .is_none_or(|conversation| event_matches_conversation(&event.payload, conversation))
        })
        .map(|e| {
            json!({
                "id": e.id,
                "payload": serde_json::to_value(&e.payload).unwrap_or_default(),
            })
        })
        .collect();

    let active_stream: Vec<serde_json::Value> = {
        let mgr = state.run_manager.lock().await;
        let mut stream_filter = conversation.map(StreamConversationFilter::new);
        mgr.recent_events(&session_id)
            .into_iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
            .filter(|value| {
                stream_filter
                    .as_mut()
                    .is_none_or(|filter| filter.matches(value))
            })
            .collect()
    };

    if active_stream.is_empty() {
        Json(json!(filtered))
    } else {
        Json(json!({
            "events": filtered,
            "active_stream": active_stream,
        }))
    }
}
