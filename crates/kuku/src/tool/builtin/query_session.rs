use std::collections::VecDeque;
use std::path::Path;

use serde_json::{json, Value};

use crate::context::revert::RewindVisibility;
use crate::conversation::address::ConversationAddress;
use crate::event::history::{event_conversation, event_turn};
use crate::event::store::HistoryMarks;
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::tool::ToolResultEnvelope;
use crate::WorkspaceIoLimits;

use super::common::{
    charge_entry, ensure_active, file_read_error, is_io_limit, FileCancellation, ScanDeadline,
};

const MAX_RESULT_CHARS: usize = 8_000;
const MAX_EVENT_CONTENT_CHARS: usize = 500;
const DEFAULT_LIMIT: usize = 20;

pub(crate) fn query_session_definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: "query_session".to_string(),
        description: "Look up earlier Session events, such as an Agent's tool calls.".to_string(),
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
        result.seen = true;
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
                result.push(format_event(event));
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
    entries: VecDeque<(String, usize)>,
    size: usize,
    count: usize,
    truncated: bool,
    seen: bool,
    scan_limit: Option<String>,
}

impl Matches {
    fn push(&mut self, display: String) {
        let size = display.chars().count() + 2;
        self.entries.push_front((display, size));
        self.size += size;
        self.count += 1;
        while self.entries.len() > 1 && self.size + 4 > MAX_RESULT_CHARS {
            self.size -= self.entries.pop_back().unwrap().1;
            self.truncated = true;
        }
    }

    fn render(self) -> ToolResultEnvelope {
        let mut output = if self.seen {
            String::from("[\n")
        } else {
            String::from("[")
        };
        for (index, (entry, _)) in self.entries.iter().enumerate() {
            if index > 0 {
                output.push_str(",\n");
            }
            output.push_str(entry);
        }
        if self.truncated {
            output.push_str("\n... (truncated, total output cap reached)");
        }
        output.push_str(if self.seen { "\n]" } else { "]" });
        if let Some(reason) = &self.scan_limit {
            output.push_str(&format!(
                "\nSession scan incomplete ({reason}). Use after or a Turn range to narrow it."
            ));
        }
        ToolResultEnvelope {
            status: "ok".into(),
            summary: format!("{} events returned", self.count),
            model_content: output,
            truncated: self.truncated || self.scan_limit.is_some(),
            structured: self
                .scan_limit
                .map(|reason| json!({"scan_complete":false,"scan_limit": reason})),
            faults: Default::default(),
            supplied_full_write: false,
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

fn format_event(event: &crate::event::StoredEvent) -> String {
    let mut json = serde_json::json!({
        "id": event.id,
        "kind": event.payload.kind_name(),
    });
    if let Ok(Value::Object(fields)) = serde_json::to_value(&event.payload) {
        for (key, value) in fields {
            let display = truncate_value(&key, value);
            json[key] = display;
        }
    }
    truncate_json_string(&serde_json::to_string(&json).unwrap_or_default())
}

fn truncate_value(key: &str, mut value: Value) -> Value {
    if matches!(key, "text" | "model_content" | "summary" | "content") {
        if let Value::String(text) = &mut value {
            if let Some((end, _)) = text.char_indices().nth(MAX_EVENT_CONTENT_CHARS) {
                text.truncate(end);
                text.push_str("...(truncated)");
            }
        }
    }
    value
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
