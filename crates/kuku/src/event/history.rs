use std::ops::Range;

use super::{EventPayload, StoredEvent};

pub(crate) fn history_cut(event: &StoredEvent) -> Option<(&str, u64)> {
    let EventPayload::Unknown(value) = &event.payload else {
        return None;
    };
    if value["kind"] != "runtime.operation" {
        return None;
    }
    let record = &value["record"];
    let first_hidden = match record["operation"].as_str()? {
        "message_rewind" => record["target_event_id"].as_u64()?,
        "history_jump" => record["target_event_id"].as_u64()?.checked_add(1)?,
        _ => return None,
    };
    (first_hidden > 0 && first_hidden <= event.id)
        .then_some((record["address"].as_str()?, first_hidden))
}

pub(crate) fn hidden_history<'a>(
    controls: impl DoubleEndedIterator<Item = &'a StoredEvent>,
    address: &str,
) -> Vec<Range<u64>> {
    let mut ranges = Vec::new();
    let mut head = u64::MAX;
    for event in controls.rev() {
        if event.id > head {
            continue;
        }
        if let Some((agent, first_hidden)) = history_cut(event) {
            if agent == address {
                ranges.push(first_hidden..event.id);
                head = first_hidden - 1;
            }
        }
    }
    ranges.reverse();
    ranges
}

pub(crate) fn hidden_range(ranges: &[Range<u64>], id: u64) -> Option<&Range<u64>> {
    ranges
        .get(ranges.partition_point(|range| range.end <= id))
        .filter(|range| range.contains(&id))
}

pub(crate) fn event_turn(payload: &EventPayload) -> Option<u64> {
    match payload {
        EventPayload::ContextSources { turn, .. }
        | EventPayload::ContextSkills { turn, .. }
        | EventPayload::ModelResponse { turn, .. }
        | EventPayload::ModelError { turn, .. }
        | EventPayload::ModelRecovery { turn, .. }
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

pub(crate) fn event_conversation(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::ModelResponse { conversation, .. }
        | EventPayload::ModelError { conversation, .. }
        | EventPayload::ToolCall { conversation, .. }
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
        EventPayload::ModelRecovery { conversation, .. } => Some(conversation.as_str()),
        _ => None,
    }
}

impl EventPayload {
    /// Agent address carried explicitly by this event, when available.
    pub fn agent_address(&self) -> Option<&str> {
        match self {
            Self::Unknown(value)
                if value["kind"] == "runtime.operation"
                    && value["record"]["operation"] == "job_finished" =>
            {
                value["record"]["agent"].as_str()
            }
            Self::Unknown(value) => value["agent"]
                .as_str()
                .or_else(|| value["conversation"].as_str())
                .or_else(|| value["record"]["address"].as_str())
                .or_else(|| value["record"]["input"]["target"].as_str()),
            payload => event_conversation(payload),
        }
    }
}
