use crate::error::Result;
use crate::query::types::{Run, RunState, UiEvent, WorkspaceAccess};

fn ready_after(barrier: &mut WorkspaceAccess, access: WorkspaceAccess) -> bool {
    if barrier.conflicts_with(access) {
        *barrier = (*barrier).max(access);
        return false;
    }
    true
}

impl Run {
    pub(super) async fn try_process_queued_call(&mut self) -> Result<Option<UiEvent>> {
        let mut barrier = self.active_workspace_access();
        let RunState::Pending(pending) = &self.state else {
            return Ok(None);
        };
        if self.slots.len() >= pending.query.runtime_limits.max_tools_per_agent
            || !self.slots.is_empty() && !pending.resumed_permission_requests.is_empty()
        {
            return Ok(None);
        }
        // Skill discovery and reads must see the result of any earlier Skill load.
        let skill_call_running = pending
            .skill_bundle_call
            .as_ref()
            .is_some_and(|id| self.slots.contains_key(id));
        let index = pending.queued_tool_calls.iter().position(|queued| {
            let access =
                crate::query::runtime_tools::workspace_access(pending, &queued.tool_call.name);
            (!skill_call_running || !crate::query::helpers::is_skill_tool(&queued.tool_call.name))
                && ready_after(&mut barrier, access)
        });
        let Some(index) = index else { return Ok(None) };
        let RunState::Pending(mut pending) =
            std::mem::replace(&mut self.state, RunState::Done(None))
        else {
            unreachable!()
        };
        let queued = pending
            .queued_tool_calls
            .remove(index)
            .expect("selected queue index");
        pending.queued_tool_calls.push_front(queued);
        self.advance_from_pending(pending).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::ProviderToolCall;
    use crate::query::run::tests::make_test_pending;
    use crate::query::types::{ExecSlot, PermissionRequest, QueuedToolCall, ToolKind};

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
            preparation: None,
        });
        pending.queued_tool_calls.push_back(QueuedToolCall {
            tool_call: ProviderToolCall {
                id: "tool_resumed_find".to_string(),
                name: "find_files".to_string(),
                args: serde_json::json!({}),
                index: 1,
            },
            display_summary: "find files".to_string(),
            preparation: None,
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
                workspace: None,
                arguments: None,
            });

        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            "tool_active_command".to_string(),
            ExecSlot {
                tool_call_id: "tool_active_command".to_string(),
                conversation: None,
                kind: ToolKind::Command { pid: None },
                workspace_access: WorkspaceAccess::Exclusive,
                job: None,
                label: "active command".to_string(),
                cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
            },
        );
        let mut run = Run {
            agent: crate::conversation::address::ConversationAddress::MAIN,
            cleanup_context: None,
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
