use std::collections::VecDeque;
use std::path::Path;

use serde_json::{Value, json};

use crate::WorkspaceIoLimits;
use crate::context::revert::RewindVisibility;
use crate::conversation::address::ConversationAddress;
use crate::event::history::{event_conversation, event_turn};
use crate::event::store::HistoryMarks;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::tool::ToolResultEnvelope;

use super::common::{
    FileCancellation, ScanDeadline, charge_entry, ensure_active, file_read_error, is_io_limit,
};

const MAX_RESULT_CHARS: usize = 8_000;
const MAX_EVENT_BYTES: usize = 600;
const RESULT_METADATA_BYTES: usize = 512;
const DEFAULT_LIMIT: usize = 20;

pub(crate) fn query_session_definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: "query_session".to_string(),
        description: "Look up earlier Session events. Returns JSON with events, omitted_events, and scan_complete.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "search": { "type": "string", "description": "Text to search for in event content" },
                "kind": { "type": "string", "description": "Filter by event kind" },
                "conversation": { "type": "string", "description": "Filter by Agent address" },
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

pub(crate) fn query_session(
    args: &Value,
    events_path: &Path,
    limits: WorkspaceIoLimits,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    match run_query(args, events_path, limits, cancelled) {
        Ok(matches) => matches.render(),
        Err(crate::Error::Io(error)) => file_read_error(Path::new("Session journal"), error),
        Err(e) => ToolResultEnvelope::error(
            format!("query_session failed: {e}"),
            format!("error querying session: {e}"),
        ),
    }
}

fn run_query(
    args: &Value,
    events_path: &Path,
    limits: WorkspaceIoLimits,
    cancelled: &FileCancellation<'_>,
) -> Result<Matches, crate::error::Error> {
    let filters = QueryFilters::new(args);
    let deadline = ScanDeadline::new(limits.scan_timeout_ms);
    let mut entries_left = limits.scan_entries;
    let history = if filters.skip_rolled_back || filters.conversation.is_some() {
        EventStore::query_history_marks(events_path, filters.conversation.is_some())?
    } else {
        HistoryMarks::default()
    };
    let visibility = RewindVisibility::new(&history.controls);
    let mut result = Matches::default();
    let mut position = None;
    let mut turn = 0usize;
    let mut seen_turn_end = false;

    'history: loop {
        ensure_active(cancelled)?;
        let page = EventStore::read_page_before(events_path, position, 256)?;
        let Some(first) = page.first() else {
            break;
        };
        position = Some(first.id);
        for event in page.iter().rev() {
            if event.id <= filters.after || result.count >= filters.limit {
                break 'history;
            }
            if let Err(error) = deadline
                .check()
                .and_then(|()| charge_entry(&mut entries_left, cancelled))
            {
                if !is_io_limit(&error) {
                    return Err(error.into());
                }
                result.scan_limit = Some(error.to_string());
                break 'history;
            }
            if filters.skip_rolled_back && !visible_event(event, &history, &visibility) {
                continue;
            }
            let age = turn;
            if matches!(
                event.payload,
                EventPayload::TurnCompleted { .. }
                    | EventPayload::TurnCancelled { .. }
                    | EventPayload::TurnInterrupted { .. }
            ) {
                turn += usize::from(seen_turn_end);
                seen_turn_end = true;
            }
            if filters.to_turn.is_some_and(|maximum| age > maximum) {
                break 'history;
            }
            if age >= filters.from_turn && filters.matches(&event.payload, &history) {
                result.push(event);
            }
        }
    }
    Ok(result)
}

struct QueryFilters<'a> {
    kind: Option<&'a str>,
    conversation: Option<&'a str>,
    search: Option<String>,
    skip_rolled_back: bool,
    after: u64,
    from_turn: usize,
    to_turn: Option<usize>,
    limit: usize,
}

impl<'a> QueryFilters<'a> {
    fn new(args: &'a Value) -> Self {
        Self {
            kind: args["kind"].as_str(),
            conversation: args["conversation"].as_str(),
            search: args["search"].as_str().map(str::to_lowercase),
            skip_rolled_back: args["skip_rolled_back"].as_bool().unwrap_or(true),
            after: args["after"].as_u64().unwrap_or_default(),
            from_turn: args["from_turn"].as_u64().unwrap_or_default() as usize,
            to_turn: args["to_turn"].as_u64().map(|turn| turn as usize),
            limit: args["limit"].as_u64().unwrap_or(DEFAULT_LIMIT as u64) as usize,
        }
    }

    fn matches(&self, payload: &EventPayload, history: &HistoryMarks) -> bool {
        self.kind.map_or_else(
            || !matches!(payload, EventPayload::ContextSkills { .. }),
            |kind| payload.kind_name() == kind,
        ) && self.conversation.is_none_or(|conversation| {
            let owner = event_conversation(payload).or_else(|| {
                event_turn(payload)
                    .and_then(|turn| history.turn_owners.get(&turn))
                    .map(|owners| owners.first.as_str())
            });
            owner.map_or_else(
                || {
                    conversation == ConversationAddress::MAIN.as_str()
                        && unscoped_main_event(payload)
                },
                |owner| owner == conversation,
            )
        }) && self.search.as_ref().is_none_or(|query| {
            let mut encoded = serde_json::to_string(payload).unwrap_or_default();
            if encoded.is_ascii() {
                encoded.make_ascii_lowercase();
                encoded.contains(query)
            } else {
                encoded.to_lowercase().contains(query)
            }
        })
    }
}

#[derive(Default)]
struct Matches {
    entries: VecDeque<Value>,
    size: usize,
    count: usize,
    abbreviated: bool,
    scan_limit: Option<String>,
}

impl Matches {
    fn push(&mut self, event: &StoredEvent) {
        self.count += 1;
        if self.entries.len() + 1 < self.count {
            return;
        }
        let (event, bytes) = format_event(event);
        if self.size + bytes + 1 > MAX_RESULT_CHARS - RESULT_METADATA_BYTES {
            return;
        }
        self.abbreviated |= event["abbreviated"] == true;
        self.entries.push_front(event);
        self.size += bytes + 1;
    }

    fn render(self) -> ToolResultEnvelope {
        let returned = self.entries.len();
        let omitted_events = self.count - returned;
        let truncated = omitted_events != 0 || self.abbreviated || self.scan_limit.is_some();
        let output = json!({
            "events": self.entries,
            "omitted_events": omitted_events,
            "scan_complete": self.scan_limit.is_none(),
            "scan_limit": self.scan_limit,
        });
        let summary = format!("{returned} events returned");
        let text = output.to_string();
        if truncated {
            ToolResultEnvelope::ok_truncated(summary, text, output)
        } else {
            ToolResultEnvelope::ok(summary, text, output)
        }
    }
}

fn visible_event(
    event: &StoredEvent,
    history: &HistoryMarks,
    visibility: &RewindVisibility,
) -> bool {
    let owner = event_conversation(&event.payload)
        .or_else(|| match &event.payload {
            EventPayload::Unknown(value) => value["record"]["address"]
                .as_str()
                .or_else(|| value["agent"].as_str()),
            _ => None,
        })
        .or_else(|| {
            event_turn(&event.payload)
                .and_then(|turn| history.turn_owners.get(&turn))
                .map(|owners| owners.last.as_str())
        });
    visibility.includes(event, owner)
}

fn unscoped_main_event(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::SessionCreated { .. }
            | EventPayload::ModelResponse { .. }
            | EventPayload::ModelError { .. }
            | EventPayload::ModelRecovery { .. }
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

fn format_event(event: &StoredEvent) -> (Value, usize) {
    let kind = event.payload.kind_name();
    let mut body = match serde_json::to_value(&event.payload) {
        Ok(Value::Object(fields)) => Value::Object(fields),
        _ => json!({}),
    };
    body["id"] = json!(event.id);
    body["kind"] = json!(kind);
    let bytes = body.to_string().len();
    if bytes <= MAX_EVENT_BYTES {
        return (body, bytes);
    }

    let kind = if serde_json::to_string(kind).expect("kind is a string").len() <= 256 {
        kind
    } else {
        &kind[..kind.floor_char_boundary(80)]
    };
    let mut preview = json!({"id": event.id, "kind": kind, "abbreviated": true});
    for field in ["turn", "tool_call_id"] {
        if let Some(value) = body
            .get(field)
            .or_else(|| body["record"].get(field))
            .filter(|value| value.is_u64() || value.as_str().is_some_and(|text| text.len() <= 256))
        {
            preview[field] = value.clone();
            if preview.to_string().len() > MAX_EVENT_BYTES {
                preview
                    .as_object_mut()
                    .expect("event preview is an object")
                    .remove(field);
            }
        }
    }
    if let Some((field, content)) = ["text", "model_content", "summary", "content"]
        .into_iter()
        .find_map(|field| body[field].as_str().map(|text| (field, text)))
    {
        let end = content.floor_char_boundary(400);
        preview[field] = json!(&content[..end]);
        if preview.to_string().len() > MAX_EVENT_BYTES {
            preview
                .as_object_mut()
                .expect("event preview is an object")
                .remove(field);
        }
    }
    let bytes = preview.to_string().len();
    (preview, bytes)
}

#[cfg(test)]
#[path = "query_session/tests.rs"]
mod tests;
