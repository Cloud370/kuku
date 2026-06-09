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

fn stream_event_matches_conversation(value: &serde_json::Value, conversation: &str) -> bool {
    value
        .get("conversation")
        .and_then(|item| item.as_str())
        .is_some_and(|value| value == conversation)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::stream_event_matches_conversation;

    #[test]
    fn stream_conversation_filter_rejects_unscoped_live_events() {
        let event = json!({"event": "line", "text": "unscoped output"});

        assert!(!stream_event_matches_conversation(&event, "review"));
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
                )
            }
        },
    };

    let events_path = match kuku::session::session_events_path(home, &workspace, &session_id) {
        Ok(p) => p,
        Err(_) => {
            return Json(
                json!({"ok": false, "code": "session_not_found", "message": "session not found"}),
            )
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
            conversation.is_none_or(|conversation| {
                event_conversation(&event.payload).is_none_or(|value| value == conversation)
            })
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
        mgr.recent_events(&session_id)
            .into_iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
            .filter(|value| {
                conversation.is_none_or(|conversation| {
                    stream_event_matches_conversation(value, conversation)
                })
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
