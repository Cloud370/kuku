use std::collections::BTreeMap;

use crate::event::{EventPayload, StoredEvent};

use super::message::{CanonicalMessage, MessageBlock, ToolResult, ToolUse};

struct PendingToolCall {
    index: u64,
    tool: String,
    args: serde_json::Value,
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
pub fn rebuild_history(events: &[StoredEvent]) -> Vec<CanonicalMessage> {
    let mut messages = Vec::new();
    let mut current_group = ResponseGroup::default();

    for event in events {
        match &event.payload {
            EventPayload::UserInput { text, .. } => {
                flush_group(&mut messages, &mut current_group);
                messages.push(CanonicalMessage::user_text(text.clone()));
            }
            EventPayload::ModelResponse {
                request_id,
                text,
                thinking,
                ..
            } => {
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
                request_id,
                tool_call_id,
                index,
                tool,
                args,
                ..
            } => {
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
                tool_call_id,
                status,
                summary,
                model_content,
                structured,
                truncated,
                ..
            } => {
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
            EventPayload::TurnEnd { .. } => flush_group(&mut messages, &mut current_group),
            EventPayload::SessionMeta { .. }
            | EventPayload::TurnStart { .. }
            | EventPayload::ModelRequest { .. }
            | EventPayload::ModelError { .. }
            | EventPayload::PolicyLoaded { .. }
            | EventPayload::PermissionRequest { .. }
            | EventPayload::PermissionDecision { .. }
            | EventPayload::HandoffTrigger { .. }
            | EventPayload::Handoff { .. }
            | EventPayload::Unknown(_) => {}
        }
    }

    flush_group(&mut messages, &mut current_group);
    messages
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

#[cfg(test)]
mod tests {
    use crate::context::{CanonicalMessage, MessageBlock, ToolResult, ToolUse};
    use crate::event::{EventPayload, StoredEvent};
    use serde_json::json;

    use super::rebuild_history;

    fn event(id: u64, payload: EventPayload) -> StoredEvent {
        StoredEvent { id, payload }
    }

    fn user_input(id: u64, turn: u64, text: &str) -> StoredEvent {
        event(
            id,
            EventPayload::UserInput {
                turn,
                ts: "2026-05-13T00:00:00Z".to_string(),
                text: text.to_string(),
            },
        )
    }

    fn model_response(
        id: u64,
        turn: u64,
        request_id: &str,
        text: &str,
        stop_reason: &str,
        tool_call_count: Option<u64>,
    ) -> StoredEvent {
        event(
            id,
            EventPayload::ModelResponse {
                turn,
                ts: "2026-05-13T00:00:01Z".to_string(),
                request_id: request_id.to_string(),
                text: text.to_string(),
                thinking: None,
                stop_reason: stop_reason.to_string(),
                tool_call_count,
                usage: json!({"input_tokens": 10}),
            },
        )
    }

    fn tool_call(
        id: u64,
        turn: u64,
        request_id: &str,
        tool_call_id: &str,
        index: u64,
        tool: &str,
    ) -> StoredEvent {
        event(
            id,
            EventPayload::ToolCall {
                turn,
                ts: "2026-05-13T00:00:02Z".to_string(),
                tool_call_id: tool_call_id.to_string(),
                request_id: request_id.to_string(),
                index,
                tool: tool.to_string(),
                args: json!({"name": tool}),
            },
        )
    }

    fn tool_result(id: u64, turn: u64, tool_call_id: &str, model_content: &str) -> StoredEvent {
        event(
            id,
            EventPayload::ToolResult {
                turn,
                ts: "2026-05-13T00:00:03Z".to_string(),
                tool_call_id: tool_call_id.to_string(),
                status: "ok".to_string(),
                summary: format!("{tool_call_id} summary"),
                model_content: model_content.to_string(),
                truncated: false,
                structured: None,
            },
        )
    }

    fn turn_end(id: u64, turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnEnd {
                turn,
                ts: "2026-05-13T00:00:04Z".to_string(),
            },
        )
    }

    #[test]
    fn preserves_stream_order_across_multiple_response_groups() {
        let events = vec![
            user_input(1, 1, "first"),
            model_response(2, 1, "req_2", "first answer", "end_turn", None),
            turn_end(3, 1),
            user_input(4, 2, "second"),
            model_response(5, 2, "req_10", "second answer", "end_turn", None),
            turn_end(6, 2),
        ];

        assert_eq!(
            rebuild_history(&events),
            vec![
                CanonicalMessage::user_text("first"),
                CanonicalMessage::assistant(vec![MessageBlock::Text("first answer".to_string())]),
                CanonicalMessage::user_text("second"),
                CanonicalMessage::assistant(vec![MessageBlock::Text("second answer".to_string())]),
            ]
        );
    }

    #[test]
    fn preserves_multiple_response_groups_inside_one_turn() {
        let events = vec![
            user_input(1, 1, "inspect"),
            model_response(2, 1, "req_1", "I will inspect.", "tool_use", Some(1)),
            tool_call(3, 1, "req_1", "tool_1", 0, "read"),
            tool_result(4, 1, "tool_1", "read output"),
            model_response(5, 1, "req_2", "Done.", "end_turn", None),
            turn_end(6, 1),
        ];

        assert_eq!(
            rebuild_history(&events),
            vec![
                CanonicalMessage::user_text("inspect"),
                CanonicalMessage::assistant(vec![
                    MessageBlock::Text("I will inspect.".to_string()),
                    MessageBlock::ToolUse(ToolUse {
                        id: "tool_1".to_string(),
                        name: "read".to_string(),
                        args: json!({"name": "read"}),
                    }),
                ]),
                CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                    tool_call_id: "tool_1".to_string(),
                    status: "ok".to_string(),
                    summary: "tool_1 summary".to_string(),
                    model_content: "read output".to_string(),
                    structured: None,
                    truncated: false,
                })]),
                CanonicalMessage::assistant(vec![MessageBlock::Text("Done.".to_string())]),
            ]
        );
    }

    #[test]
    fn orders_tool_use_and_tool_results_by_tool_call_index() {
        let events = vec![
            user_input(1, 1, "inspect"),
            model_response(2, 1, "req_1", "I will inspect.", "tool_use", Some(2)),
            tool_call(3, 1, "req_1", "tool_b", 1, "grep"),
            tool_call(4, 1, "req_1", "tool_a", 0, "read"),
            tool_result(5, 1, "tool_b", "grep output"),
            tool_result(6, 1, "tool_a", "read output"),
            turn_end(7, 1),
        ];

        assert_eq!(
            rebuild_history(&events),
            vec![
                CanonicalMessage::user_text("inspect"),
                CanonicalMessage::assistant(vec![
                    MessageBlock::Text("I will inspect.".to_string()),
                    MessageBlock::ToolUse(ToolUse {
                        id: "tool_a".to_string(),
                        name: "read".to_string(),
                        args: json!({"name": "read"}),
                    }),
                    MessageBlock::ToolUse(ToolUse {
                        id: "tool_b".to_string(),
                        name: "grep".to_string(),
                        args: json!({"name": "grep"}),
                    }),
                ]),
                CanonicalMessage::user(vec![
                    MessageBlock::ToolResult(ToolResult {
                        tool_call_id: "tool_a".to_string(),
                        status: "ok".to_string(),
                        summary: "tool_a summary".to_string(),
                        model_content: "read output".to_string(),
                        structured: None,
                        truncated: false,
                    }),
                    MessageBlock::ToolResult(ToolResult {
                        tool_call_id: "tool_b".to_string(),
                        status: "ok".to_string(),
                        summary: "tool_b summary".to_string(),
                        model_content: "grep output".to_string(),
                        structured: None,
                        truncated: false,
                    }),
                ]),
            ]
        );
    }

    #[test]
    fn synthesizes_cancelled_results_for_unmatched_tool_calls() {
        let events = vec![
            user_input(1, 1, "inspect"),
            model_response(2, 1, "req_1", "I will inspect.", "tool_use", Some(2)),
            tool_call(3, 1, "req_1", "tool_a", 0, "read"),
            tool_call(4, 1, "req_1", "tool_b", 1, "grep"),
            tool_result(5, 1, "tool_a", "read output"),
            turn_end(6, 1),
        ];

        assert_eq!(
            rebuild_history(&events),
            vec![
                CanonicalMessage::user_text("inspect"),
                CanonicalMessage::assistant(vec![
                    MessageBlock::Text("I will inspect.".to_string()),
                    MessageBlock::ToolUse(ToolUse {
                        id: "tool_a".to_string(),
                        name: "read".to_string(),
                        args: json!({"name": "read"}),
                    }),
                    MessageBlock::ToolUse(ToolUse {
                        id: "tool_b".to_string(),
                        name: "grep".to_string(),
                        args: json!({"name": "grep"}),
                    }),
                ]),
                CanonicalMessage::user(vec![
                    MessageBlock::ToolResult(ToolResult {
                        tool_call_id: "tool_a".to_string(),
                        status: "ok".to_string(),
                        summary: "tool_a summary".to_string(),
                        model_content: "read output".to_string(),
                        structured: None,
                        truncated: false,
                    }),
                    MessageBlock::ToolResult(ToolResult {
                        tool_call_id: "tool_b".to_string(),
                        status: "cancelled".to_string(),
                        summary: "tool result missing during replay".to_string(),
                        model_content: "tool call was cancelled before producing a result"
                            .to_string(),
                        structured: None,
                        truncated: false,
                    }),
                ]),
            ]
        );
    }

    #[test]
    fn preserves_thinking_in_response_group() {
        let events = vec![
            user_input(1, 1, "hi"),
            event(
                2,
                EventPayload::ModelResponse {
                    turn: 1,
                    ts: "2026-05-13T00:00:01Z".to_string(),
                    request_id: "req_1".to_string(),
                    text: "Hello!".to_string(),
                    thinking: Some("The user said hi".to_string()),
                    stop_reason: "end_turn".to_string(),
                    tool_call_count: None,
                    usage: json!({"input_tokens": 10}),
                },
            ),
            turn_end(3, 1),
        ];

        assert_eq!(
            rebuild_history(&events),
            vec![
                CanonicalMessage::user_text("hi"),
                CanonicalMessage::assistant(vec![
                    MessageBlock::Thinking("The user said hi".to_string()),
                    MessageBlock::Text("Hello!".to_string()),
                ]),
            ]
        );
    }

    #[test]
    fn preserves_thinking_with_tool_calls() {
        let events = vec![
            user_input(1, 1, "inspect"),
            event(
                2,
                EventPayload::ModelResponse {
                    turn: 1,
                    ts: "2026-05-13T00:00:01Z".to_string(),
                    request_id: "req_1".to_string(),
                    text: "I will inspect.".to_string(),
                    thinking: Some("Need to read the file first".to_string()),
                    stop_reason: "tool_use".to_string(),
                    tool_call_count: Some(1),
                    usage: json!({"input_tokens": 10}),
                },
            ),
            tool_call(3, 1, "req_1", "tool_1", 0, "read"),
            tool_result(4, 1, "tool_1", "read output"),
            event(
                5,
                EventPayload::ModelResponse {
                    turn: 1,
                    ts: "2026-05-13T00:00:05Z".to_string(),
                    request_id: "req_2".to_string(),
                    text: "Done.".to_string(),
                    thinking: Some("File looks good".to_string()),
                    stop_reason: "end_turn".to_string(),
                    tool_call_count: None,
                    usage: json!({"input_tokens": 12}),
                },
            ),
            turn_end(6, 1),
        ];

        assert_eq!(
            rebuild_history(&events),
            vec![
                CanonicalMessage::user_text("inspect"),
                CanonicalMessage::assistant(vec![
                    MessageBlock::Thinking("Need to read the file first".to_string()),
                    MessageBlock::Text("I will inspect.".to_string()),
                    MessageBlock::ToolUse(ToolUse {
                        id: "tool_1".to_string(),
                        name: "read".to_string(),
                        args: json!({"name": "read"}),
                    }),
                ]),
                CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                    tool_call_id: "tool_1".to_string(),
                    status: "ok".to_string(),
                    summary: "tool_1 summary".to_string(),
                    model_content: "read output".to_string(),
                    structured: None,
                    truncated: false,
                })]),
                CanonicalMessage::assistant(vec![
                    MessageBlock::Thinking("File looks good".to_string()),
                    MessageBlock::Text("Done.".to_string()),
                ]),
            ]
        );
    }
}
