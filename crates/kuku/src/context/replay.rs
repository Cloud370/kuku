use std::collections::BTreeMap;
use std::collections::HashSet;

use crate::conversation::address::ConversationAddress;

use super::message::{CanonicalMessage, MessageBlock, ToolResult, ToolUse};
use super::revert::filter_rolled_back_events;
use crate::event::{EventPayload, ModelStopReason, StoredEvent};

pub(crate) const SUPPLIED_FULL_WRITE: &str = "supplied_full_write";

struct PendingToolCall {
    index: u64,
    tool: String,
    args: serde_json::Value,
}

#[derive(Hash, Eq, PartialEq)]
struct ToolCallKey {
    conversation: Option<String>,
    turn: u64,
    tool_call_id: String,
}

#[derive(Default)]
struct ResponseGroup {
    request_id: Option<String>,
    assistant_source_event_id: Option<u64>,
    tool_result_source_event_id: Option<u64>,
    text: Option<String>,
    thinking: Option<String>,
    tool_calls: BTreeMap<String, PendingToolCall>,
    tool_results: BTreeMap<String, ToolResult>,
    replay: Vec<MessageBlock>,
    suppress_thinking: bool,
}

/// Reconstruct conversation history messages from a session's stored events.
/// Returns `(handoff_summary, messages)` where `handoff_summary` is the text
/// from the most recent Handoff event (if any), and `messages` contains the
/// most recent `keep_turns` turns before that handoff plus later events.
pub fn rebuild_history(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
) -> (Option<String>, Vec<CanonicalMessage>) {
    let (summary, messages) = rebuild_history_internal(events, conversation, false);
    (
        summary,
        messages.into_iter().map(|(message, _)| message).collect(),
    )
}

pub(crate) fn rebuild_history_for_provider(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
) -> (Option<String>, Vec<CanonicalMessage>) {
    let (summary, messages) = rebuild_history_internal(events, conversation, false);
    (
        summary,
        messages.into_iter().map(|(message, _)| message).collect(),
    )
}

pub(crate) fn rebuild_history_for_provider_with_provenance(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
) -> (Option<String>, Vec<(CanonicalMessage, u64)>) {
    rebuild_history_internal(events, conversation, true)
}

pub(crate) fn effective_snapshot_events<'a>(
    events: &'a [StoredEvent],
    conversation: &ConversationAddress,
) -> Vec<&'a StoredEvent> {
    let filtered = filter_rolled_back_events(events);
    let session_runtime = is_session_runtime(events);
    let suppressed_turns = if session_runtime {
        HashSet::new()
    } else {
        suppressed_turns(&filtered, conversation)
    };
    let turns = turn_conversations(&filtered);
    let handoff_pos = filtered
        .iter()
        .enumerate()
        .rfind(|(_, event)| matches!(event.payload, EventPayload::Handoff { turn, .. } if unscoped_turn_belongs_to_conversation(turn, conversation, &turns)));
    let start_idx = handoff_pos.map_or(0, |(index, _)| {
        let keep_turns = match &filtered[index].payload {
            EventPayload::Handoff { keep_turns, .. } => *keep_turns,
            _ => 0,
        };
        handoff_start_index(&filtered, index, keep_turns, conversation)
    });

    let effective = &filtered[start_idx..];
    let latest_response = if session_runtime {
        None
    } else {
        effective
            .iter()
            .rfind(|event| {
                matches!(&event.payload, EventPayload::ModelResponse {
                turn,
                stop_reason: None | Some(ModelStopReason::EndTurn | ModelStopReason::ToolUse),
                ..
            } if !suppressed_turns.contains(turn))
                    && event_belongs_to_history_conversation(&event.payload, conversation)
            })
            .map(|event| event.id)
    };
    effective
        .iter()
        .copied()
        .filter(|event| {
            if !event_belongs_to_history_conversation(&event.payload, conversation) {
                return false;
            }
            if event_turn(&event.payload).is_some_and(|turn| suppressed_turns.contains(&turn)) {
                return false;
            }
            matches!(
                &event.payload,
                EventPayload::ToolResult {
                    status,
                    structured: Some(structured),
                    ..
                } if status == "ok"
                    && ((structured["kind"] == "file_content" && structured["cached"] != true)
                        || ((session_runtime || latest_response.is_some_and(|id| event.id < id))
                            && supplied_file_version(structured).is_some()))
            )
        })
        .collect()
}

pub(crate) fn supplied_file_version(structured: &serde_json::Value) -> Option<(&str, &str)> {
    if structured[SUPPLIED_FULL_WRITE] != true
        || structured["kind"] != "file_write"
        || structured["commit_state"] != "committed"
    {
        return None;
    }
    Some((
        structured["canonical_path"].as_str()?,
        structured["content_hash_after"].as_str()?,
    ))
}

pub(crate) fn render_agent_message(text: &str, from: &str) -> String {
    let from = from
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;");
    let text = text.replace('&', "&amp;").replace('<', "&lt;");
    format!("<kuku_agent_message from=\"{from}\">\n{text}\n</kuku_agent_message>")
}

fn rebuild_history_internal(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
    render_agent_sender: bool,
) -> (Option<String>, Vec<(CanonicalMessage, u64)>) {
    let filtered = filter_rolled_back_events(events);
    let suppressed_turns = if is_session_runtime(events) {
        HashSet::new()
    } else {
        suppressed_turns(&filtered, conversation)
    };
    let turn_conversations = turn_conversations(&filtered);

    let handoff_pos = filtered
        .iter()
        .enumerate()
        .rfind(|(_, e)| matches!(e.payload, EventPayload::Handoff { turn, .. } if unscoped_turn_belongs_to_conversation(turn, conversation, &turn_conversations)));

    let (summary, effective) = match handoff_pos {
        Some((idx, event)) => {
            let (summary, keep_turns) = match &event.payload {
                EventPayload::Handoff {
                    summary,
                    keep_turns,
                    ..
                } => (Some(summary.clone()), *keep_turns),
                _ => (None, 0),
            };
            let start_idx = handoff_start_index(&filtered, idx, keep_turns, conversation);
            (summary, &filtered[start_idx..])
        }
        None => (None, filtered.as_slice()),
    };

    let response_messages = effective
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ModelResponse {
                turn,
                conversation: source,
                request_id,
                text,
                ..
            } if source.as_ref().map_or_else(
                || unscoped_turn_belongs_to_conversation(*turn, conversation, &turn_conversations),
                |source| source == conversation.as_str(),
            ) =>
            {
                Some((*turn, request_id.as_str(), text.as_str()))
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut messages = Vec::new();
    let mut current_group = ResponseGroup::default();
    let mut seen_tool_call_keys = HashSet::new();

    for event in effective {
        if event_turn(&event.payload).is_some_and(|turn| suppressed_turns.contains(&turn))
            && event_belongs_to_history_conversation(&event.payload, conversation)
        {
            continue;
        }
        if matches!(
            &event.payload,
            EventPayload::ModelResponse {
                stop_reason: Some(reason),
                ..
            } if !matches!(
                reason,
                crate::event::ModelStopReason::EndTurn | crate::event::ModelStopReason::ToolUse
            )
        ) {
            continue;
        }
        match &event.payload {
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.provider_replay"
                    && value["agent"] == conversation.as_str()
                    && !value["turn"]
                        .as_u64()
                        .is_some_and(|turn| suppressed_turns.contains(&turn)) =>
            {
                let Some(request_id) = value["request_id"].as_str() else {
                    continue;
                };
                if current_group
                    .request_id
                    .as_ref()
                    .is_some_and(|active| active != request_id)
                {
                    flush_group(&mut messages, &mut current_group);
                }
                current_group.request_id = Some(request_id.into());
                current_group.assistant_source_event_id = Some(event.id);
                if let Ok(protocol) = serde_json::from_value(value["protocol"].clone()) {
                    let replay = value["value"].clone();
                    if protocol == crate::config::ProviderFormat::Anthropic
                        && replay["role"] == "assistant"
                        && replay["content"].as_array().is_some_and(Vec::is_empty)
                    {
                        current_group.suppress_thinking = true;
                    } else {
                        current_group.replay.push(MessageBlock::ProviderReplay {
                            protocol,
                            value: replay,
                        });
                    }
                }
            }
            EventPayload::MessageUser {
                conversation: event_conversation,
                text,
                from,
                ..
            } if event_conversation == conversation.as_str() => {
                flush_group(&mut messages, &mut current_group);
                let provider_text = match from.as_deref() {
                    Some(from) if render_agent_sender => render_agent_message(text, from),
                    _ => text.clone(),
                };
                messages.push((CanonicalMessage::user_text(provider_text), event.id));
            }
            EventPayload::ModelResponse {
                conversation: event_conversation,
                turn,
                request_id,
                text,
                thinking,
                ..
            } if event_conversation.as_ref().map_or_else(
                || unscoped_turn_belongs_to_conversation(*turn, conversation, &turn_conversations),
                |event_conversation| event_conversation == conversation.as_str(),
            ) =>
            {
                if current_group
                    .request_id
                    .as_ref()
                    .is_some_and(|active| active != request_id)
                {
                    flush_group(&mut messages, &mut current_group);
                }
                current_group.request_id = Some(request_id.clone());
                current_group.assistant_source_event_id = Some(event.id);
                current_group.text = Some(text.clone());
                current_group.thinking = thinking.clone();
            }
            EventPayload::ToolCall {
                conversation: None,
                turn,
                request_id,
                tool_call_id,
                index,
                tool,
                args,
                ..
            } if conversation.is_main() => {
                let key = ToolCallKey {
                    conversation: None,
                    turn: *turn,
                    tool_call_id: tool_call_id.clone(),
                };
                if request_id == tool_call_id
                    && permission_metadata_args(args)
                    && seen_tool_call_keys.contains(&key)
                {
                    continue;
                }
                seen_tool_call_keys.insert(key);
                if current_group.request_id.as_ref() != Some(request_id) {
                    flush_group(&mut messages, &mut current_group);
                    current_group.request_id = Some(request_id.clone());
                }
                current_group.assistant_source_event_id = Some(event.id);
                current_group.tool_calls.insert(
                    tool_call_id.clone(),
                    PendingToolCall {
                        index: *index,
                        tool: tool.clone(),
                        args: args.clone(),
                    },
                );
            }
            EventPayload::ToolCall {
                conversation: Some(event_conversation),
                turn,
                request_id,
                tool_call_id,
                index,
                tool,
                args,
                ..
            } if event_conversation == conversation.as_str() => {
                let key = ToolCallKey {
                    conversation: Some(event_conversation.clone()),
                    turn: *turn,
                    tool_call_id: tool_call_id.clone(),
                };
                if request_id == tool_call_id
                    && permission_metadata_args(args)
                    && seen_tool_call_keys.contains(&key)
                {
                    continue;
                }
                seen_tool_call_keys.insert(key);
                if current_group.request_id.as_ref() != Some(request_id) {
                    flush_group(&mut messages, &mut current_group);
                    current_group.request_id = Some(request_id.clone());
                }
                current_group.assistant_source_event_id = Some(event.id);
                current_group.tool_calls.insert(
                    tool_call_id.clone(),
                    PendingToolCall {
                        index: *index,
                        tool: tool.clone(),
                        args: args.clone(),
                    },
                );
            }
            EventPayload::ToolResult {
                conversation: None,
                tool_call_id,
                status,
                summary,
                model_content,
                structured,
                truncated,
                ..
            } if conversation.is_main() => {
                current_group.tool_results.insert(
                    tool_call_id.clone(),
                    ToolResult {
                        tool_call_id: tool_call_id.clone(),
                        status: status.clone(),
                        summary: summary.clone(),
                        model_content: model_content.clone(),
                        structured: structured.clone(),
                        truncated: *truncated,
                    },
                );
                current_group.tool_result_source_event_id = Some(event.id);
            }
            EventPayload::ToolResult {
                conversation: Some(event_conversation),
                tool_call_id,
                status,
                summary,
                model_content,
                structured,
                truncated,
                ..
            } if event_conversation == conversation.as_str() => {
                current_group.tool_results.insert(
                    tool_call_id.clone(),
                    ToolResult {
                        tool_call_id: tool_call_id.clone(),
                        status: status.clone(),
                        summary: summary.clone(),
                        model_content: model_content.clone(),
                        structured: structured.clone(),
                        truncated: *truncated,
                    },
                );
                current_group.tool_result_source_event_id = Some(event.id);
            }
            EventPayload::TurnCompleted {
                conversation: event_conversation,
                ..
            } if event_conversation == conversation.as_str() => {
                flush_group(&mut messages, &mut current_group)
            }
            EventPayload::MessageAssistant {
                conversation: event_conversation,
                turn,
                message_id,
                text,
                ..
            } if event_conversation == conversation.as_str()
                && !response_messages.contains(&(*turn, message_id.as_str(), text.as_str())) =>
            {
                flush_group(&mut messages, &mut current_group);
                messages.push((
                    CanonicalMessage::assistant(vec![MessageBlock::Text(text.clone())]),
                    event.id,
                ));
            }
            EventPayload::SessionCreated { .. }
            | EventPayload::ContextSources { .. }
            | EventPayload::ContextSkills { .. }
            | EventPayload::PromptSnapshot { .. }
            | EventPayload::TurnStarted { .. }
            | EventPayload::ModelError { .. }
            | EventPayload::ModelRecovery { .. }
            | EventPayload::PermissionRequested { .. }
            | EventPayload::PermissionAllow { .. }
            | EventPayload::PermissionDeny { .. }
            | EventPayload::Handoff { .. }
            | EventPayload::TurnCancelled { .. }
            | EventPayload::TurnInterrupted { .. }
            | EventPayload::Unknown(_) => {}
            _ => {}
        }
    }

    flush_group(&mut messages, &mut current_group);
    (summary, messages)
}

fn is_session_runtime(events: &[StoredEvent]) -> bool {
    events.iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.operation" && value["record"]["operation"] == "agent_created"))
}

fn handoff_start_index(
    events: &[&StoredEvent],
    handoff_idx: usize,
    keep_turns: usize,
    conversation: &ConversationAddress,
) -> usize {
    if keep_turns == 0 {
        return handoff_idx + 1;
    }
    let turns = turn_conversations(events);
    let mut retained = HashSet::new();
    for event in events[..=handoff_idx].iter().rev() {
        let Some(turn) = event_turn(&event.payload) else {
            continue;
        };
        if !unscoped_turn_belongs_to_conversation(turn, conversation, &turns) {
            continue;
        }
        retained.insert(turn);
        if retained.len() == keep_turns {
            return events[..=handoff_idx]
                .iter()
                .position(|event| event_turn(&event.payload) == Some(turn))
                .unwrap_or(0);
        }
    }
    0
}

fn event_turn(payload: &EventPayload) -> Option<u64> {
    match payload {
        EventPayload::MessageUser { turn, .. }
        | EventPayload::ModelResponse { turn, .. }
        | EventPayload::ToolCall { turn, .. }
        | EventPayload::ToolResult { turn, .. }
        | EventPayload::TurnStarted { turn, .. }
        | EventPayload::TurnCompleted { turn, .. }
        | EventPayload::TurnCancelled { turn, .. }
        | EventPayload::TurnInterrupted { turn, .. }
        | EventPayload::ContextSources { turn, .. }
        | EventPayload::ContextSkills { turn, .. }
        | EventPayload::ModelError { turn, .. }
        | EventPayload::ModelRecovery { turn, .. }
        | EventPayload::PermissionRequested { turn, .. }
        | EventPayload::PermissionAllow { turn, .. }
        | EventPayload::PermissionDeny { turn, .. }
        | EventPayload::Handoff { turn, .. } => Some(*turn),
        EventPayload::SessionCreated { .. }
        | EventPayload::PromptSnapshot { .. }
        | EventPayload::MessageAssistant { .. }
        | EventPayload::Unknown(_) => None,
    }
}

fn suppressed_turns(events: &[&StoredEvent], conversation: &ConversationAddress) -> HashSet<u64> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TurnCancelled {
                conversation: event_conversation,
                turn,
                ..
            }
            | EventPayload::TurnInterrupted {
                conversation: event_conversation,
                turn,
                ..
            } if event_conversation == conversation.as_str() => Some(*turn),
            _ => None,
        })
        .collect()
}

fn turn_conversations(events: &[&StoredEvent]) -> BTreeMap<u64, HashSet<String>> {
    let mut turns = BTreeMap::<u64, HashSet<String>>::new();
    for event in events {
        match &event.payload {
            EventPayload::TurnStarted {
                turn, conversation, ..
            }
            | EventPayload::MessageUser {
                turn, conversation, ..
            }
            | EventPayload::MessageAssistant {
                turn, conversation, ..
            }
            | EventPayload::TurnCompleted {
                turn, conversation, ..
            }
            | EventPayload::TurnCancelled {
                turn, conversation, ..
            }
            | EventPayload::TurnInterrupted {
                turn, conversation, ..
            }
            | EventPayload::ContextSkills {
                turn, conversation, ..
            }
            | EventPayload::PromptSnapshot {
                turn, conversation, ..
            } => {
                turns.entry(*turn).or_default().insert(conversation.clone());
            }
            EventPayload::ToolCall {
                turn, conversation, ..
            }
            | EventPayload::ToolResult {
                turn, conversation, ..
            } => {
                turns.entry(*turn).or_default().insert(
                    conversation
                        .clone()
                        .unwrap_or_else(|| ConversationAddress::MAIN.as_str().to_string()),
                );
            }
            _ => {}
        }
    }
    turns
}

fn unscoped_turn_belongs_to_conversation(
    turn: u64,
    conversation: &ConversationAddress,
    turn_conversations: &BTreeMap<u64, HashSet<String>>,
) -> bool {
    turn_conversations.get(&turn).map_or_else(
        || conversation.is_main(),
        |conversations| conversations.contains(conversation.as_str()),
    )
}

fn event_belongs_to_history_conversation(
    payload: &EventPayload,
    conversation: &ConversationAddress,
) -> bool {
    match payload {
        EventPayload::ModelResponse {
            conversation: event_conversation,
            ..
        }
        | EventPayload::ModelError {
            conversation: event_conversation,
            ..
        } => event_conversation.as_ref().map_or_else(
            || conversation.is_main(),
            |value| value == conversation.as_str(),
        ),
        EventPayload::ContextSources { .. } | EventPayload::Handoff { .. } => {
            conversation.is_main()
        }
        EventPayload::ModelRecovery {
            conversation: event_conversation,
            ..
        } => event_conversation == conversation.as_str(),
        EventPayload::ToolCall {
            conversation: None, ..
        }
        | EventPayload::ToolResult {
            conversation: None, ..
        } => conversation.is_main(),
        EventPayload::ToolCall {
            conversation: Some(event_conversation),
            ..
        }
        | EventPayload::ToolResult {
            conversation: Some(event_conversation),
            ..
        }
        | EventPayload::MessageUser {
            conversation: event_conversation,
            ..
        }
        | EventPayload::MessageAssistant {
            conversation: event_conversation,
            ..
        }
        | EventPayload::ContextSkills {
            conversation: event_conversation,
            ..
        } => event_conversation == conversation.as_str(),
        _ => false,
    }
}

fn flush_group(messages: &mut Vec<(CanonicalMessage, u64)>, group: &mut ResponseGroup) {
    if group.request_id.is_none()
        && group.text.is_none()
        && group.tool_calls.is_empty()
        && group.tool_results.is_empty()
    {
        return;
    }

    let mut calls = std::mem::take(&mut group.tool_calls)
        .into_iter()
        .collect::<Vec<_>>();
    calls.sort_by_key(|(_, call)| call.index);

    let mut assistant_blocks = Vec::new();
    assistant_blocks.append(&mut group.replay);
    if group.suppress_thinking {
        group.thinking = None;
    } else if let Some(thinking) = group.thinking.take().filter(|t| !t.is_empty()) {
        assistant_blocks.push(MessageBlock::Thinking(thinking));
    }
    if let Some(text) = group.text.take().filter(|text| !text.is_empty()) {
        assistant_blocks.push(MessageBlock::Text(text));
    }

    for (tool_call_id, call) in &calls {
        assistant_blocks.push(MessageBlock::ToolUse(ToolUse {
            id: tool_call_id.clone(),
            name: call.tool.clone(),
            args: call.args.clone(),
        }));
    }

    if !assistant_blocks.is_empty() {
        let source_event_id = group
            .assistant_source_event_id
            .expect("canonical assistant messages have a source event");
        messages.push((
            CanonicalMessage::assistant(assistant_blocks),
            source_event_id,
        ));
    }

    let mut result_blocks = Vec::new();
    let mut results = std::mem::take(&mut group.tool_results);
    for (tool_call_id, _) in calls {
        let result = results
            .remove(&tool_call_id)
            .unwrap_or_else(|| cancelled_tool_result(&tool_call_id));
        result_blocks.push(MessageBlock::ToolResult(result));
    }

    if !result_blocks.is_empty() {
        let source_event_id = group
            .tool_result_source_event_id
            .or(group.assistant_source_event_id)
            .expect("canonical tool results have a source event");
        messages.push((CanonicalMessage::user(result_blocks), source_event_id));
    }

    group.request_id = None;
    group.assistant_source_event_id = None;
    group.tool_result_source_event_id = None;
    group.suppress_thinking = false;
}

fn cancelled_tool_result(tool_call_id: &str) -> ToolResult {
    ToolResult {
        tool_call_id: tool_call_id.to_string(),
        status: "cancelled".to_string(),
        summary: "tool result missing during replay".to_string(),
        model_content: "tool call was cancelled before producing a result".to_string(),
        structured: None,
        truncated: false,
    }
}

fn permission_metadata_args(args: &serde_json::Value) -> bool {
    let Some(object) = args.as_object() else {
        return false;
    };
    object.len() == 4
        && object.contains_key("risk")
        && object.contains_key("summary")
        && object.contains_key("candidate")
        && object.contains_key("source")
}

#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;
