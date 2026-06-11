use crate::error::Result;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::query::helpers::now_timestamp;
use crate::query::types::PendingRun;
use crate::tool::ToolDefinition;

pub(super) fn has_permission_decision(events: &[StoredEvent], tool_call_id: &str) -> bool {
    events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::PermissionAllow { tool_call_id: id, .. }
                | EventPayload::PermissionDeny { tool_call_id: id, .. }
                if id == tool_call_id
        )
    })
}

pub(super) fn persist_blocked_tool_result(
    events_path: &std::path::Path,
    turn: u64,
    tool_call_id: &str,
    summary: &str,
) -> Result<()> {
    let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
    let mut store = EventStore::open(events_path)?;
    store.append(EventPayload::ToolResult {
        turn,
        ts: now_timestamp()?,
        conversation: None,
        tool_call_id: tool_call_id.to_string(),
        status: "blocked".to_string(),
        summary: summary.to_string(),
        model_content: String::new(),
        truncated: false,
        files_read: Vec::new(),
        files_changed: Vec::new(),
        commands_run: Vec::new(),
        memory_changed: None,
        structured: Some(blocked),
    })?;
    Ok(())
}

pub(super) fn find_tool_definition<'a>(
    pending: &'a PendingRun,
    name: &str,
) -> Option<&'a ToolDefinition> {
    pending
        .resolved
        .as_ref()
        .and_then(|resolved| resolved.registry.iter().find(|tool| tool.name == name))
}
