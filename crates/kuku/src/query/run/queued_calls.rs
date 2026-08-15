use crate::error::Result;

use super::find_tool_definition;
use super::helpers::{has_permission_decision, persist_blocked_tool_result};
use crate::query::helpers::{
    append_permission_decision, append_permission_request, display_summary, is_inline_skill_tool,
    permission_candidate, permission_rule, resolved_tool_available,
};
use crate::query::slots::requires_workspace_ordering;
use crate::query::tool_exec::run_tool_pre_hooks;
use crate::query::types::{
    PendingPermission, PermissionChoice, PermissionRequest, QueuedToolCall, Run, RunState, UiEvent,
};

impl Run {
    pub(super) async fn try_process_queued_call(&mut self) -> Result<Option<UiEvent>> {
        let has_active_workspace_ordered_slot = self.has_active_workspace_ordered_slot();
        let (queue_index, tool_call_id, tool_name) = match &self.state {
            RunState::Pending(pending) => {
                let queue_index = if has_active_workspace_ordered_slot
                    && pending.resumed_permission_requests.is_empty()
                {
                    pending.queued_tool_calls.iter().position(|queued| {
                        let name = queued.tool_call.name.as_str();
                        !requires_workspace_ordering(name)
                            && name != "agent"
                            && !(is_inline_skill_tool(name)
                                && resolved_tool_available(pending, name))
                    })
                } else if !has_active_workspace_ordered_slot {
                    (!pending.queued_tool_calls.is_empty()).then_some(0)
                } else {
                    None
                };
                let Some(queue_index) = queue_index else {
                    return Ok(None);
                };
                let queued = &pending.queued_tool_calls[queue_index];
                (
                    queue_index,
                    queued.tool_call.id.clone(),
                    queued.tool_call.name.clone(),
                )
            }
            _ => return Ok(None),
        };
        let resumed_request = match &mut self.state {
            RunState::Pending(pending) => pending.take_resumed_permission_request(&tool_call_id),
            _ => return Ok(None),
        };
        if let Some(request) = resumed_request {
            let state = std::mem::replace(&mut self.state, RunState::Done(None));
            if let RunState::Pending(pending) = state {
                self.state = RunState::WaitingForPermission(Box::new(PendingPermission {
                    pending: *pending,
                    request: request.clone(),
                }));
                return Ok(Some(UiEvent::PermissionRequested { request }));
            }
        }

        let pending = match &mut self.state {
            RunState::Pending(p) => p.as_mut(),
            _ => return Ok(None),
        };
        if requires_workspace_ordering(&tool_name) && has_active_workspace_ordered_slot {
            return Ok(None);
        }
        if tool_name == "agent"
            || (is_inline_skill_tool(&tool_name) && resolved_tool_available(pending, &tool_name))
        {
            return Ok(None);
        }
        crate::query::provider::ensure_resolved(pending)?;
        let queued = match pending.queued_tool_calls.get(queue_index) {
            Some(q) => q,
            None => return Ok(None),
        };

        let policy = crate::permission::load_project_policy(&pending.policy_path)?;
        let prior_events = crate::event::EventStore::replay(&pending.events_path)?;
        let session_grants = crate::permission::recover_session_grants(&prior_events);

        let definition = match find_tool_definition(pending, &queued.tool_call.name) {
            Some(d) => d,
            None => {
                let QueuedToolCall { tool_call, .. } =
                    pending.queued_tool_calls.remove(queue_index).unwrap();
                return Ok(Some(UiEvent::Error {
                    code: "unknown_tool".to_string(),
                    message: format!("unknown tool: {}", tool_call.name),
                }));
            }
        };
        let candidate = permission_candidate(
            &pending.kuku_home,
            &pending.workspace,
            &queued.tool_call.name,
            &queued.tool_call.args,
        );
        let decision = crate::permission::decide_tool_call(
            &queued.tool_call.name,
            &definition.risk,
            &candidate,
            &policy,
            &session_grants,
        );

        match decision.kind {
            crate::permission::GateDecisionKind::Ask => Ok(None),
            crate::permission::GateDecisionKind::Allow => {
                if !matches!(decision.source, crate::permission::GateSource::TrustPosture) {
                    let choice = crate::query::helpers::gate_choice(&decision.source);
                    if !has_permission_decision(&prior_events, &queued.tool_call.id) {
                        append_permission_decision(
                            &pending.events_path,
                            pending.turn,
                            &queued.tool_call.id,
                            choice,
                            crate::query::helpers::gate_source_name(decision.source),
                            &permission_rule(
                                &pending.kuku_home,
                                &pending.workspace,
                                &queued.tool_call.name,
                                &queued.tool_call.args,
                            ),
                        )?;
                    }
                }
                let QueuedToolCall {
                    tool_call,
                    display_summary,
                } = pending.queued_tool_calls.remove(queue_index).unwrap();
                let hook_result = run_tool_pre_hooks(
                    &mut *pending,
                    &tool_call.name,
                    &tool_call.args,
                    &tool_call.id,
                )
                .await?;
                if let Some(block) = hook_result.block {
                    let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
                    pending.record_tool_call(&tool_call.name);
                    persist_blocked_tool_result(
                        &pending.events_path,
                        pending.turn,
                        &tool_call.id,
                        &block.reason,
                    )?;
                    return Ok(Some(UiEvent::ToolEnd {
                        id: tool_call.id,
                        status: "blocked".to_string(),
                        summary: block.reason,
                        model_content: None,
                        result: Some(blocked),
                    }));
                }
                pending.record_tool_call(&tool_call.name);
                let (slot, tool_kind) = crate::query::slots::dispatch_tool_slot(
                    crate::query::slots::SlotDispatchArgs {
                        tool_name: tool_call.name.clone(),
                        tool_id: tool_call.id.clone(),
                        conversation: (!pending.conversation.is_main())
                            .then(|| pending.conversation.clone()),
                        args: hook_result.args,
                        summary: display_summary.clone(),
                        workspace: pending.workspace.clone(),
                        kuku_home: pending.kuku_home.clone(),
                        prior_events: prior_events.clone(),
                        event_tx: self.slot_event_tx.clone(),
                        config: pending.config.clone(),
                        catalog: pending.catalog.clone(),
                        events_path: pending.events_path.clone(),
                        workspace_capability: pending.workspace_capability.clone(),
                    },
                );
                self.slots.insert(slot.tool_call_id.clone(), slot);
                Ok(Some(UiEvent::ToolStart {
                    id: tool_call.id,
                    tool: tool_call.name,
                    summary: display_summary,
                    kind: tool_kind,
                }))
            }
            crate::permission::GateDecisionKind::Deny => {
                let risk = definition.risk.clone();
                let QueuedToolCall { tool_call, .. } =
                    pending.queued_tool_calls.remove(queue_index).unwrap();
                append_permission_request(
                    &pending.events_path,
                    &pending.conversation,
                    pending.turn,
                    &PermissionRequest {
                        id: tool_call.id.clone(),
                        conversation: pending.conversation.clone(),
                        turn: pending.turn,
                        tool_call_id: tool_call.id.clone(),
                        tool: tool_call.name.clone(),
                        risk,
                        summary: display_summary(&tool_call.name, &tool_call.args, None),
                        candidate,
                        source: crate::query::helpers::gate_source_name(decision.source)
                            .to_string(),
                    },
                )?;
                append_permission_decision(
                    &pending.events_path,
                    pending.turn,
                    &tool_call.id,
                    PermissionChoice::Deny,
                    crate::query::helpers::gate_source_name(decision.source),
                    &permission_rule(
                        &pending.kuku_home,
                        &pending.workspace,
                        &tool_call.name,
                        &tool_call.args,
                    ),
                )?;
                pending.record_tool_denied(&tool_call.name);
                let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
                persist_blocked_tool_result(
                    &pending.events_path,
                    pending.turn,
                    &tool_call.id,
                    "permission denied",
                )?;
                Ok(Some(UiEvent::ToolEnd {
                    id: tool_call.id,
                    status: "blocked".to_string(),
                    summary: "permission denied".to_string(),
                    model_content: None,
                    result: Some(blocked),
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::ProviderToolCall;
    use crate::query::run::tests::make_test_pending;
    use crate::query::types::{ExecSlot, ToolKind};

    #[tokio::test]
    async fn active_ordered_slot_does_not_bypass_to_later_resumed_permission() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let mut pending = make_test_pending(
            events_path,
            dir.path(),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.queued_tool_calls.push_back(QueuedToolCall {
            tool_call: ProviderToolCall {
                id: "tool_ordered_read".to_string(),
                name: "read_file".to_string(),
                args: serde_json::json!({"path": "visible.txt"}),
                index: 0,
            },
            display_summary: "read visible.txt".to_string(),
        });
        pending.queued_tool_calls.push_back(QueuedToolCall {
            tool_call: ProviderToolCall {
                id: "tool_resumed_find".to_string(),
                name: "find_files".to_string(),
                args: serde_json::json!({}),
                index: 1,
            },
            display_summary: "find files".to_string(),
        });
        pending
            .resumed_permission_requests
            .push_back(PermissionRequest {
                id: "request_resumed_find".to_string(),
                conversation: crate::conversation::address::ConversationAddress::MAIN,
                turn: 1,
                tool_call_id: "tool_resumed_find".to_string(),
                tool: "find_files".to_string(),
                risk: "read".to_string(),
                summary: "find files".to_string(),
                candidate: "find_files".to_string(),
                source: "resume".to_string(),
            });

        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            "tool_active_command".to_string(),
            ExecSlot {
                tool_call_id: "tool_active_command".to_string(),
                conversation: None,
                kind: ToolKind::Command { pid: None },
                workspace_ordered: true,
                label: "active command".to_string(),
                cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
                nested_permissions: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
            },
        );
        let mut run = Run {
            session_id: "test".to_string(),
            state: RunState::Pending(Box::new(pending)),
            slots,
            slot_event_tx,
            slot_event_rx,
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        };

        let event = run.try_process_queued_call().await.unwrap();

        assert!(event.is_none());
        assert!(matches!(
            &run.state,
            RunState::Pending(pending)
                if pending.queued_tool_calls.len() == 2
                    && pending.queued_tool_calls.front().unwrap().tool_call.id
                        == "tool_ordered_read"
                    && pending.resumed_permission_requests.front().unwrap().tool_call_id
                        == "tool_resumed_find"
        ));
    }
}
