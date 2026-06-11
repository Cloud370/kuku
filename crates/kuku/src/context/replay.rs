use std::collections::BTreeMap;
use std::collections::HashSet;

use crate::conversation::address::ConversationAddress;

use super::message::{CanonicalMessage, MessageBlock, ToolResult, ToolUse};
use super::revert::filter_rolled_back_events;
use crate::event::{EventPayload, StoredEvent};

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
    text: Option<String>,
    thinking: Option<String>,
    tool_calls: BTreeMap<String, PendingToolCall>,
    tool_results: BTreeMap<String, ToolResult>,
}

/// Reconstruct conversation history messages from a session's stored events.
/// Returns `(handoff_summary, messages)` where `handoff_summary` is the text
/// from the most recent Handoff event (if any), and `messages` contains the
/// most recent `keep_turns` turns before that handoff plus later events.
pub fn rebuild_history(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
) -> (Option<String>, Vec<CanonicalMessage>) {
    rebuild_history_internal(events, conversation, false)
}

pub(crate) fn rebuild_history_for_provider(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
) -> (Option<String>, Vec<CanonicalMessage>) {
    rebuild_history_internal(events, conversation, true)
}

fn rebuild_history_internal(
    events: &[StoredEvent],
    conversation: &ConversationAddress,
    wrap_delegated_prompts: bool,
) -> (Option<String>, Vec<CanonicalMessage>) {
    let filtered = filter_rolled_back_events(events);
    let suppressed_turns = suppressed_turns(&filtered, conversation);
    let turn_conversations = turn_conversations(&filtered);

    let handoff_pos = filtered
        .iter()
        .enumerate()
        .rfind(|(_, e)| matches!(e.payload, EventPayload::Handoff { .. }));

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
            let start_idx = handoff_start_index(&filtered, idx, keep_turns);
            (summary, &filtered[start_idx..])
        }
        None => (None, filtered.as_slice()),
    };

    let mut messages = Vec::new();
    let mut current_group = ResponseGroup::default();
    let mut seen_tool_call_keys = HashSet::new();

    for event in effective {
        if event_turn(&event.payload).is_some_and(|turn| suppressed_turns.contains(&turn))
            && event_belongs_to_history_conversation(&event.payload, conversation)
        {
            continue;
        }
        match &event.payload {
            EventPayload::MessageUser {
                conversation: event_conversation,
                text,
                from,
                via_tool_call_id,
                ..
            } if event_conversation == conversation.as_str() => {
                flush_group(&mut messages, &mut current_group);
                let provider_text =
                    if wrap_delegated_prompts && from.is_some() && via_tool_call_id.is_some() {
                        wrap_delegated_prompt_for_provider(text)
                    } else {
                        text.clone()
                    };
                messages.push(CanonicalMessage::user_text(provider_text));
            }
            EventPayload::ModelResponse {
                turn,
                request_id,
                text,
                thinking,
                ..
            } if conversation.is_main()
                && unscoped_turn_belongs_to_conversation(
                    *turn,
                    conversation,
                    &turn_conversations,
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
            }
            EventPayload::TurnCompleted {
                conversation: event_conversation,
                ..
            } if event_conversation == conversation.as_str() => {
                flush_group(&mut messages, &mut current_group)
            }
            EventPayload::MessageAssistant {
                conversation: event_conversation,
                text,
                ..
            } if event_conversation == conversation.as_str() => {
                flush_group(&mut messages, &mut current_group);
                messages.push(CanonicalMessage::assistant(vec![MessageBlock::Text(
                    text.clone(),
                )]));
            }
            EventPayload::SessionCreated { .. }
            | EventPayload::ContextSources { .. }
            | EventPayload::ContextSkills { .. }
            | EventPayload::ConversationOpened { .. }
            | EventPayload::ConversationBound { .. }
            | EventPayload::PromptSnapshot { .. }
            | EventPayload::TurnStarted { .. }
            | EventPayload::ModelError { .. }
            | EventPayload::PermissionRequested { .. }
            | EventPayload::PermissionAllow { .. }
            | EventPayload::PermissionDeny { .. }
            | EventPayload::Handoff { .. }
            | EventPayload::TurnCancelled { .. }
            | EventPayload::TurnInterrupted { .. }
            | EventPayload::ConversationRollback { .. }
            | EventPayload::ConversationRollbackUndone { .. }
            | EventPayload::Unknown(_) => {}
            _ => {}
        }
    }

    flush_group(&mut messages, &mut current_group);
    (summary, messages)
}

fn wrap_delegated_prompt_for_provider(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<kuku_delegated_prompt>\n{escaped}\n</kuku_delegated_prompt>")
}

fn handoff_start_index(events: &[&StoredEvent], handoff_idx: usize, keep_turns: usize) -> usize {
    if keep_turns == 0 {
        return handoff_idx + 1;
    }

    let mut turns_seen = 0;
    let mut last_turn = None;
    let mut scan_idx = handoff_idx;

    loop {
        if let Some(turn) = event_turn(&events[scan_idx].payload) {
            if last_turn != Some(turn) {
                turns_seen += 1;
                last_turn = Some(turn);
            }
            if turns_seen == keep_turns {
                let mut start_idx = scan_idx;
                while start_idx > 0 && event_turn(&events[start_idx - 1].payload) == Some(turn) {
                    start_idx -= 1;
                }
                return start_idx;
            }
        }

        if scan_idx == 0 {
            return 0;
        }
        scan_idx -= 1;
    }
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
        | EventPayload::PermissionRequested { turn, .. }
        | EventPayload::PermissionAllow { turn, .. }
        | EventPayload::PermissionDeny { turn, .. }
        | EventPayload::Handoff { turn, .. } => Some(*turn),
        EventPayload::SessionCreated { .. }
        | EventPayload::ConversationOpened { .. }
        | EventPayload::ConversationBound { .. }
        | EventPayload::PromptSnapshot { .. }
        | EventPayload::MessageAssistant { .. }
        | EventPayload::ConversationRollback { .. }
        | EventPayload::ConversationRollbackUndone { .. }
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
        EventPayload::ModelResponse { .. }
        | EventPayload::ModelError { .. }
        | EventPayload::ContextSources { .. }
        | EventPayload::Handoff { .. } => conversation.is_main(),
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

fn flush_group(messages: &mut Vec<CanonicalMessage>, group: &mut ResponseGroup) {
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
    if let Some(thinking) = group.thinking.take().filter(|t| !t.is_empty()) {
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
        messages.push(CanonicalMessage::assistant(assistant_blocks));
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
        messages.push(CanonicalMessage::user(result_blocks));
    }

    group.request_id = None;
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
mod tests;
