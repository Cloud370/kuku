use std::collections::BTreeMap;

use crate::event::{EventPayload, StoredEvent};

use super::revert::filter_rolled_back_events;

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
/// Returns `(handoff_summary, messages)` where `handoff_summary` is the text
/// from the most recent Handoff event (if any), and `messages` contains only
/// events after that handoff.
pub fn rebuild_history(events: &[StoredEvent]) -> (Option<String>, Vec<CanonicalMessage>) {
    let filtered = filter_rolled_back_events(events);

    let handoff_pos = filtered
        .iter()
        .enumerate()
        .rfind(|(_, e)| matches!(e.payload, EventPayload::Handoff { .. }));

    let (summary, effective) = match handoff_pos {
        Some((idx, event)) => {
            let summary = match &event.payload {
                EventPayload::Handoff { summary, .. } => Some(summary.clone()),
                _ => None,
            };
            (summary, &filtered[idx + 1..])
        }
        None => (None, filtered.as_slice()),
    };

    let mut messages = Vec::new();
    let mut current_group = ResponseGroup::default();

    for event in effective {
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
            | EventPayload::TurnRollback { .. }
            | EventPayload::TurnRollbackUndo { .. }
            | EventPayload::Unknown(_) => {}
        }
    }

    flush_group(&mut messages, &mut current_group);
    (summary, messages)
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
    use crate::context::revert::filter_rolled_back_events;
    use crate::event::RollbackScope;

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

        let (summary, history) = rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(
            history,
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

        let (summary, history) = rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(
            history,
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

        let (summary, history) = rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(
            history,
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

        let (summary, history) = rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(
            history,
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

        let (summary, history) = rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(
            history,
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

        let (summary, history) = rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(
            history,
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

    fn handoff_event(id: u64, summary: &str) -> StoredEvent {
        event(
            id,
            EventPayload::Handoff {
                ts: "2026-05-27T00:00:01Z".to_string(),
                summary: summary.to_string(),
                kept_turns: 2,
            },
        )
    }

    #[test]
    fn rebuild_history_returns_none_summary_when_no_handoff() {
        let events = vec![
            user_input(1, 1, "hello"),
            model_response(2, 1, "req_1", "hi", "end_turn", None),
            turn_end(3, 1),
        ];
        let (summary, history) = rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn rebuild_history_returns_summary_and_skips_pre_handoff() {
        let events = vec![
            user_input(1, 1, "old question"),
            model_response(2, 1, "req_1", "old answer", "end_turn", None),
            turn_end(3, 1),
            handoff_event(4, "## Goal\nDo stuff"),
            user_input(5, 2, "new question"),
            model_response(6, 2, "req_2", "new answer", "end_turn", None),
            turn_end(7, 2),
        ];
        let (summary, history) = rebuild_history(&events);
        assert_eq!(summary.as_deref(), Some("## Goal\nDo stuff"));
        assert_eq!(history.len(), 2);
        assert_eq!(history[0], CanonicalMessage::user_text("new question"));
    }

    #[test]
    fn rebuild_history_uses_last_handoff_when_multiple() {
        let events = vec![
            user_input(1, 1, "first"),
            model_response(2, 1, "req_1", "answer1", "end_turn", None),
            turn_end(3, 1),
            handoff_event(4, "first summary"),
            user_input(5, 2, "second"),
            model_response(6, 2, "req_2", "answer2", "end_turn", None),
            turn_end(7, 2),
            handoff_event(8, "second summary"),
            user_input(9, 3, "third"),
            model_response(10, 3, "req_3", "answer3", "end_turn", None),
            turn_end(11, 3),
        ];
        let (summary, history) = rebuild_history(&events);
        assert_eq!(summary.as_deref(), Some("second summary"));
        assert_eq!(history.len(), 2);
        assert_eq!(history[0], CanonicalMessage::user_text("third"));
    }

    fn turn_start(id: u64, turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnStart {
                turn,
                ts: "t".to_string(),
            },
        )
    }

    fn rollback(id: u64, turn: u64, target_turn: u64, scope: RollbackScope) -> StoredEvent {
        event(
            id,
            EventPayload::TurnRollback {
                turn,
                ts: "t".to_string(),
                target_turn,
                scope,
            },
        )
    }

    fn rollback_undo(id: u64, turn: u64, rb_id: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnRollbackUndo {
                turn,
                ts: "t".to_string(),
                rollback_event_id: rb_id,
            },
        )
    }

    fn extract_user_texts<'a>(events: &[&'a StoredEvent]) -> Vec<&'a str> {
        events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::UserInput { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn no_rollback_returns_all() {
        let events = vec![turn_start(1, 1), user_input(2, 1, "a"), turn_end(3, 1)];
        assert_eq!(filter_rolled_back_events(&events).len(), 3);
    }

    #[test]
    fn both_scope_skips_target_and_later_turns() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            turn_start(4, 2),
            user_input(5, 2, "b"),
            turn_end(6, 2),
            turn_start(7, 3),
            user_input(8, 3, "c"),
            turn_end(9, 3),
            rollback(10, 4, 2, RollbackScope::Both),
        ];
        let f = filter_rolled_back_events(&events);
        assert_eq!(extract_user_texts(&f), vec!["a"]);
    }

    #[test]
    fn conversation_only_skips_turns() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            turn_start(4, 2),
            user_input(5, 2, "b"),
            turn_end(6, 2),
            rollback(7, 3, 2, RollbackScope::ConversationOnly),
        ];
        let f = filter_rolled_back_events(&events);
        assert_eq!(extract_user_texts(&f), vec!["a"]);
    }

    #[test]
    fn files_only_keeps_conversation() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            turn_start(4, 2),
            user_input(5, 2, "b"),
            turn_end(6, 2),
            rollback(7, 3, 2, RollbackScope::FilesOnly),
        ];
        let f = filter_rolled_back_events(&events);
        assert_eq!(extract_user_texts(&f), vec!["a", "b"]);
    }

    #[test]
    fn undo_restores_events() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            turn_start(4, 2),
            user_input(5, 2, "b"),
            turn_end(6, 2),
            rollback(7, 3, 2, RollbackScope::ConversationOnly),
            rollback_undo(8, 4, 7),
        ];
        let f = filter_rolled_back_events(&events);
        assert_eq!(extract_user_texts(&f), vec!["a", "b"]);
    }

    #[test]
    fn rollback_before_handoff_removes_handoff() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            handoff_event(4, "old summary"),
            turn_start(5, 2),
            user_input(6, 2, "b"),
            turn_end(7, 2),
            rollback(8, 3, 1, RollbackScope::ConversationOnly),
        ];
        let (summary, msgs) = rebuild_history(&events);
        assert!(summary.is_none());
        assert!(msgs.is_empty());
    }

    #[test]
    fn rollback_after_handoff_keeps_summary() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            handoff_event(4, "summary of turn 1"),
            turn_start(5, 2),
            user_input(6, 2, "b"),
            turn_end(7, 2),
            turn_start(8, 3),
            user_input(9, 3, "c"),
            turn_end(10, 3),
            rollback(11, 4, 3, RollbackScope::ConversationOnly),
        ];
        let (summary, msgs) = rebuild_history(&events);
        assert_eq!(summary.as_deref(), Some("summary of turn 1"));
        let texts: Vec<_> = msgs
            .iter()
            .filter_map(|m| {
                if let MessageBlock::Text(t) = &m.blocks[0] {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(texts, vec!["b"]);
    }

    #[test]
    fn consecutive_rollbacks_last_wins() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            turn_start(4, 2),
            user_input(5, 2, "b"),
            turn_end(6, 2),
            turn_start(7, 3),
            user_input(8, 3, "c"),
            turn_end(9, 3),
            rollback(10, 4, 2, RollbackScope::ConversationOnly),
            rollback(11, 5, 3, RollbackScope::ConversationOnly),
        ];
        let f = filter_rolled_back_events(&events);
        assert_eq!(extract_user_texts(&f), vec!["a", "b"]);
    }

    #[test]
    fn undo_first_of_two_rollbacks_second_still_active() {
        let events = vec![
            turn_start(1, 1),
            user_input(2, 1, "a"),
            turn_end(3, 1),
            turn_start(4, 2),
            user_input(5, 2, "b"),
            turn_end(6, 2),
            turn_start(7, 3),
            user_input(8, 3, "c"),
            turn_end(9, 3),
            rollback(10, 4, 2, RollbackScope::ConversationOnly),
            rollback(11, 5, 3, RollbackScope::ConversationOnly),
            rollback_undo(12, 6, 10),
        ];
        let f = filter_rolled_back_events(&events);
        assert_eq!(extract_user_texts(&f), vec!["a", "b"]);
    }
}
