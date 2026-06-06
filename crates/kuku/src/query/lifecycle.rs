#![allow(dead_code)]

use std::collections::HashMap;

use crate::context::revert::filter_rolled_back_events;
use crate::event::{EventPayload, StoredEvent};
use crate::provider::types::ProviderToolCall;

use super::types::PermissionRequest;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LifecycleState {
    pub(super) open_tools: Vec<OpenToolLifecycle>,
    pub(super) pending_permissions: Vec<PendingPermissionLifecycle>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingPermissionLifecycle {
    pub(super) tool_call: ProviderToolCall,
    pub(super) request: PermissionRequest,
    pub(super) turn: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OpenToolLifecycle {
    pub(super) tool_call: ProviderToolCall,
    pub(super) kind: OpenToolKind,
    pub(super) turn: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OpenToolKind {
    AllowedWithoutResult,
    DeniedWithoutResult,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq)]
struct ToolLifecycle {
    tool_call: ProviderToolCall,
    turn: u64,
    permission_request: Option<PermissionRequest>,
    permission_allowed: bool,
    permission_denied: bool,
    has_result: bool,
}

pub(super) fn reduce_lifecycle(events: &[StoredEvent]) -> LifecycleState {
    let mut tools = HashMap::<String, ToolLifecycle>::new();

    for event in filter_rolled_back_events(events) {
        match &event.payload {
            EventPayload::ToolCall {
                turn,
                tool_call_id,
                index,
                tool,
                args,
                ..
            } => {
                tools.entry(tool_call_id.clone()).or_insert_with(|| ToolLifecycle {
                    tool_call: ProviderToolCall {
                        id: tool_call_id.clone(),
                        name: tool.clone(),
                        args: args.clone(),
                        index: *index,
                    },
                    turn: *turn,
                    permission_request: None,
                    permission_allowed: false,
                    permission_denied: false,
                    has_result: false,
                });
            }
            EventPayload::PermissionRequested {
                turn,
                tool_call_id,
                tool,
                risk,
                summary,
                candidate,
                source,
                ..
            } => {
                if let Some(lifecycle) = tools.get_mut(tool_call_id) {
                    lifecycle.permission_request = Some(PermissionRequest {
                        id: tool_call_id.clone(),
                        tool_call_id: tool_call_id.clone(),
                        tool: tool.clone(),
                        risk: risk.clone(),
                        summary: summary.clone(),
                        candidate: candidate.clone(),
                        source: source.clone(),
                    });
                    lifecycle.turn = *turn;
                }
            }
            EventPayload::PermissionAllow { tool_call_id, .. } => {
                if let Some(lifecycle) = tools.get_mut(tool_call_id) {
                    lifecycle.permission_allowed = true;
                }
            }
            EventPayload::PermissionDeny { tool_call_id, .. } => {
                if let Some(lifecycle) = tools.get_mut(tool_call_id) {
                    lifecycle.permission_denied = true;
                }
            }
            EventPayload::ToolResult { tool_call_id, .. } => {
                if let Some(lifecycle) = tools.get_mut(tool_call_id) {
                    lifecycle.has_result = true;
                }
            }
            EventPayload::SessionMeta { .. }
            | EventPayload::ContextPrelude { .. }
            | EventPayload::ContextSources { .. }
            | EventPayload::TurnStart { .. }
            | EventPayload::UserInput { .. }
            | EventPayload::ModelResponse { .. }
            | EventPayload::ModelError { .. }
            | EventPayload::Handoff { .. }
            | EventPayload::TurnEnd { .. }
            | EventPayload::TurnRollback { .. }
            | EventPayload::TurnRollbackUndo { .. }
            | EventPayload::Unknown(_) => {}
        }
    }

    let mut lifecycles = tools.into_values().collect::<Vec<_>>();
    lifecycles.sort_by(|left, right| {
        (left.turn, left.tool_call.index, left.tool_call.id.as_str()).cmp(&(
            right.turn,
            right.tool_call.index,
            right.tool_call.id.as_str(),
        ))
    });

    let mut open_tools = Vec::new();
    let mut pending_permissions = Vec::new();

    for lifecycle in lifecycles {
        if lifecycle.has_result {
            continue;
        }

        if lifecycle.permission_denied {
            open_tools.push(OpenToolLifecycle {
                tool_call: lifecycle.tool_call,
                kind: OpenToolKind::DeniedWithoutResult,
                turn: lifecycle.turn,
            });
        } else if lifecycle.permission_allowed {
            open_tools.push(OpenToolLifecycle {
                tool_call: lifecycle.tool_call,
                kind: OpenToolKind::AllowedWithoutResult,
                turn: lifecycle.turn,
            });
        } else if let Some(request) = lifecycle.permission_request {
            pending_permissions.push(PendingPermissionLifecycle {
                tool_call: lifecycle.tool_call,
                request,
                turn: lifecycle.turn,
            });
        } else {
            open_tools.push(OpenToolLifecycle {
                tool_call: lifecycle.tool_call,
                kind: OpenToolKind::Interrupted,
                turn: lifecycle.turn,
            });
        }
    }

    LifecycleState {
        open_tools,
        pending_permissions,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{OpenToolKind, reduce_lifecycle};
    use crate::event::{EventPayload, RollbackScope, StoredEvent};

    fn event(id: u64, payload: EventPayload) -> StoredEvent {
        StoredEvent { id, payload }
    }

    fn turn_start(id: u64, turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnStart {
                turn,
                ts: "ts".to_string(),
            },
        )
    }

    fn tool_call(id: u64, turn: u64, tool_call_id: &str, index: u64) -> StoredEvent {
        event(
            id,
            EventPayload::ToolCall {
                turn,
                ts: "ts".to_string(),
                tool_call_id: tool_call_id.to_string(),
                request_id: format!("req_{turn}"),
                index,
                tool: "write".to_string(),
                args: json!({ "path": tool_call_id }),
            },
        )
    }

    fn permission_requested(id: u64, turn: u64, tool_call_id: &str) -> StoredEvent {
        event(
            id,
            EventPayload::PermissionRequested {
                turn,
                ts: "ts".to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool: "write".to_string(),
                risk: "modifies_files".to_string(),
                summary: format!("request {tool_call_id}"),
                candidate: format!("candidate {tool_call_id}"),
                source: "tool_policy".to_string(),
            },
        )
    }

    fn permission_allow(id: u64, turn: u64, tool_call_id: &str) -> StoredEvent {
        event(
            id,
            EventPayload::PermissionAllow {
                turn,
                ts: "ts".to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool: "write".to_string(),
                scope: "once".to_string(),
                matcher: "write".to_string(),
                source: "host".to_string(),
            },
        )
    }

    fn permission_deny(id: u64, turn: u64, tool_call_id: &str) -> StoredEvent {
        event(
            id,
            EventPayload::PermissionDeny {
                turn,
                ts: "ts".to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool: "write".to_string(),
                reason: "no".to_string(),
                source: "host".to_string(),
            },
        )
    }

    fn tool_result(id: u64, turn: u64, tool_call_id: &str) -> StoredEvent {
        event(
            id,
            EventPayload::ToolResult {
                turn,
                ts: "ts".to_string(),
                tool_call_id: tool_call_id.to_string(),
                status: "ok".to_string(),
                summary: "done".to_string(),
                model_content: "done".to_string(),
                truncated: false,
                structured: None,
            },
        )
    }

    fn rollback(id: u64, turn: u64, target_turn: u64) -> StoredEvent {
        event(
            id,
            EventPayload::TurnRollback {
                turn,
                ts: "ts".to_string(),
                target_turn,
                scope: RollbackScope::ConversationOnly,
            },
        )
    }

    #[test]
    fn pending_permission_keeps_tool_call_and_request_fields() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "tc_1", 7),
            permission_requested(3, 1, "tc_1"),
        ]);

        assert_eq!(state.open_tools.len(), 0);
        assert_eq!(state.pending_permissions.len(), 1);
        let pending = &state.pending_permissions[0];
        assert_eq!(pending.turn, 1);
        assert_eq!(pending.tool_call.id, "tc_1");
        assert_eq!(pending.tool_call.name, "write");
        assert_eq!(pending.tool_call.args, json!({ "path": "tc_1" }));
        assert_eq!(pending.tool_call.index, 7);
        assert_eq!(pending.request.id, "tc_1");
        assert_eq!(pending.request.tool_call_id, "tc_1");
        assert_eq!(pending.request.tool, "write");
        assert_eq!(pending.request.risk, "modifies_files");
        assert_eq!(pending.request.summary, "request tc_1");
        assert_eq!(pending.request.candidate, "candidate tc_1");
        assert_eq!(pending.request.source, "tool_policy");
    }

    #[test]
    fn closed_tool_is_not_returned() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "tc_1", 0),
            permission_requested(3, 1, "tc_1"),
            permission_allow(4, 1, "tc_1"),
            tool_result(5, 1, "tc_1"),
        ]);

        assert!(state.open_tools.is_empty());
        assert!(state.pending_permissions.is_empty());
    }

    #[test]
    fn denied_without_result_is_open_denied() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "tc_1", 0),
            permission_requested(3, 1, "tc_1"),
            permission_deny(4, 1, "tc_1"),
        ]);

        assert_eq!(state.pending_permissions.len(), 0);
        assert_eq!(state.open_tools.len(), 1);
        assert_eq!(state.open_tools[0].kind, OpenToolKind::DeniedWithoutResult);
        assert_eq!(state.open_tools[0].tool_call.id, "tc_1");
    }

    #[test]
    fn allowed_without_result_is_open_allowed() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "tc_1", 0),
            permission_requested(3, 1, "tc_1"),
            permission_allow(4, 1, "tc_1"),
        ]);

        assert_eq!(state.pending_permissions.len(), 0);
        assert_eq!(state.open_tools.len(), 1);
        assert_eq!(state.open_tools[0].kind, OpenToolKind::AllowedWithoutResult);
        assert_eq!(state.open_tools[0].tool_call.id, "tc_1");
    }

    #[test]
    fn multiple_pending_permissions_are_sorted_by_tool_call_index() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "second", 2),
            tool_call(3, 1, "first", 1),
            permission_requested(4, 1, "second"),
            permission_requested(5, 1, "first"),
        ]);

        let ids = state
            .pending_permissions
            .iter()
            .map(|pending| pending.tool_call.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["first", "second"]);
    }

    #[test]
    fn rollback_filtered_pending_permission_is_ignored() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "old", 0),
            permission_requested(3, 1, "old"),
            turn_start(4, 2),
            rollback(5, 2, 1),
        ]);

        assert!(state.open_tools.is_empty());
        assert!(state.pending_permissions.is_empty());
    }

    #[test]
    fn legacy_tool_without_result_is_open_interrupted() {
        let state = reduce_lifecycle(&[turn_start(1, 1), tool_call(2, 1, "tc_1", 0)]);

        assert_eq!(state.pending_permissions.len(), 0);
        assert_eq!(state.open_tools.len(), 1);
        assert_eq!(state.open_tools[0].kind, OpenToolKind::Interrupted);
        assert_eq!(state.open_tools[0].tool_call.id, "tc_1");
    }

    #[test]
    fn duplicate_tool_call_does_not_resurrect_closed_tool() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "tc_1", 0),
            tool_result(3, 1, "tc_1"),
            tool_call(4, 1, "tc_1", 0),
        ]);

        assert!(state.open_tools.is_empty());
        assert!(state.pending_permissions.is_empty());
    }

    #[test]
    fn duplicate_tool_call_does_not_drop_pending_permission() {
        let state = reduce_lifecycle(&[
            turn_start(1, 1),
            tool_call(2, 1, "tc_1", 0),
            permission_requested(3, 1, "tc_1"),
            tool_call(4, 1, "tc_1", 9),
        ]);

        assert_eq!(state.open_tools.len(), 0);
        assert_eq!(state.pending_permissions.len(), 1);
        assert_eq!(state.pending_permissions[0].tool_call.id, "tc_1");
        assert_eq!(state.pending_permissions[0].tool_call.index, 0);
        assert_eq!(state.pending_permissions[0].request.summary, "request tc_1");
    }

    #[test]
    fn repeated_indices_are_ordered_by_turn_then_index_then_id() {
        let state = reduce_lifecycle(&[
            turn_start(1, 2),
            tool_call(2, 2, "b", 0),
            tool_call(3, 2, "a", 0),
            turn_start(4, 1),
            tool_call(5, 1, "c", 0),
            permission_requested(6, 2, "b"),
            permission_requested(7, 2, "a"),
            permission_requested(8, 1, "c"),
        ]);

        let pending_ids = state
            .pending_permissions
            .iter()
            .map(|pending| pending.tool_call.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(pending_ids, vec!["c", "a", "b"]);
    }
}
