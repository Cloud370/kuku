use super::{
    append_permission_decision, append_project_allow_rule, display_summary, execute_tool_call,
    now_timestamp, permission_candidate, permission_rule, persist_blocked_tool_result,
    requires_ordered_simple_execution, run_tool_pre_hooks, Error, EventPayload, PendingPermission,
    PermissionChoice, QueuedToolCall, Result, Run, RunState, UiEvent,
};

impl Run {
    /// Apply a permission decision for a pending tool call.
    /// `parent_tool_id`: `None` for top-level, `Some(id)` for delegated permission.
    pub async fn decide(
        &mut self,
        request_id: &str,
        choice: PermissionChoice,
        parent_tool_id: Option<&str>,
    ) -> Result<Option<UiEvent>> {
        if let Some(tool_id) = parent_tool_id {
            let slot = self
                .slots
                .get_mut(tool_id)
                .ok_or_else(|| Error::PermissionRequestNotPending(request_id.to_string()))?;
            let mut map = slot.nested_permissions.lock().unwrap();
            let tx = map
                .remove(request_id)
                .ok_or_else(|| Error::PermissionRequestNotPending(request_id.to_string()))?;
            drop(map);
            let _ = tx.send(choice);
            Ok(None)
        } else {
            self.apply_choice(request_id, choice, "host").await
        }
    }

    /// Cancel a single running tool by its tool_call_id.
    pub fn cancel_tool(&mut self, tool_call_id: &str) -> bool {
        if let Some(slot) = self.slots.get(tool_call_id) {
            slot.cancel.notify_one();
            true
        } else {
            false
        }
    }

    pub(in crate::query) async fn deny_pending(&mut self) -> Result<Option<UiEvent>> {
        let request_id = match &self.state {
            RunState::WaitingForPermission(waiting) => waiting.request.id.clone(),
            _ => {
                return Err(Error::PermissionRequestNotPending(
                    "no permission request is pending".to_string(),
                ));
            }
        };
        self.apply_choice(&request_id, PermissionChoice::Deny, "runtime")
            .await
    }

    /// Cancel a pending permission without recording an allow or deny decision.
    pub fn cancel_pending_permission(&mut self, request_id: &str) -> Result<Option<UiEvent>> {
        let state = std::mem::replace(&mut self.state, RunState::Done(None));
        let mut waiting = match state {
            RunState::WaitingForPermission(waiting) if waiting.request.id == request_id => *waiting,
            other => {
                self.state = other;
                return Err(Error::PermissionRequestNotPending(request_id.to_string()));
            }
        };

        let result = match self.close_pending_permission_as_cancelled(&waiting) {
            Ok(result) => result,
            Err(error) => {
                self.state = RunState::WaitingForPermission(Box::new(waiting));
                return Err(error);
            }
        };

        let QueuedToolCall { tool_call, .. } = waiting
            .pending
            .queued_tool_calls
            .pop_front()
            .expect("PendingPermission implies a queued tool call");

        self.state = RunState::Pending(Box::new(waiting.pending));
        Ok(Some(UiEvent::ToolEnd {
            id: tool_call.id,
            status: result.status,
            summary: result.summary,
            model_content: None,
            result: result.structured,
        }))
    }

    pub(super) fn close_pending_permission_as_cancelled(
        &self,
        waiting: &PendingPermission,
    ) -> Result<crate::tool::ToolResultEnvelope> {
        let tool_call = match waiting.pending.queued_tool_calls.front() {
            Some(queued) if queued.tool_call.id == waiting.request.tool_call_id => {
                &queued.tool_call
            }
            Some(queued) => {
                let message = format!(
                    "pending permission {} expects tool call {}, but queued tool call is {}",
                    waiting.request.id, waiting.request.tool_call_id, queued.tool_call.id
                );
                return Err(Error::InvalidEventStream(message));
            }
            None => {
                return Err(Error::InvalidEventStream(format!(
                    "pending permission {} has no queued tool call",
                    waiting.request.id
                )));
            }
        };
        let result = crate::tool::ToolResultEnvelope::cancelled("permission request cancelled");
        let mut event_store = waiting.pending.event_store.clone();
        event_store.append(EventPayload::ToolResult {
            execution: waiting.pending.execution_scope().clone(),
            turn: waiting.pending.turn,
            ts: now_timestamp()?,
            conversation: None,
            tool_call_id: tool_call.id.clone(),
            status: result.status.clone(),
            summary: result.summary.clone(),
            model_content: result.model_content.clone(),
            truncated: result.truncated,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: result.structured.clone(),
        })?;
        Ok(result)
    }

    async fn apply_choice(
        &mut self,
        request_id: &str,
        choice: PermissionChoice,
        source: &str,
    ) -> Result<Option<UiEvent>> {
        let state = std::mem::replace(&mut self.state, RunState::Done(None));
        let waiting = match state {
            RunState::WaitingForPermission(waiting) if waiting.request.id == request_id => *waiting,
            other => {
                self.state = other;
                return Err(Error::PermissionRequestNotPending(request_id.to_string()));
            }
        };

        let mut pending = waiting.pending;
        let queued = pending
            .queued_tool_calls
            .pop_front()
            .expect("PendingPermission implies a queued tool call");
        let QueuedToolCall {
            request,
            tool_call,
            display_summary: queued_summary,
        } = queued;
        let rule = permission_rule(
            &pending.kuku_home,
            &pending.workspace,
            &tool_call.name,
            &tool_call.args,
        );
        if matches!(choice, PermissionChoice::Project) {
            append_project_allow_rule(
                &pending.policy_path,
                &tool_call.name,
                &permission_candidate(
                    &pending.kuku_home,
                    &pending.workspace,
                    &tool_call.name,
                    &tool_call.args,
                ),
            )?;
        }
        append_permission_decision(
            &pending.event_store,
            pending.execution_scope(),
            pending.turn,
            &tool_call.id,
            choice,
            source,
            &rule,
        )?;
        let prior_events = pending.event_store.read_all()?;
        if matches!(choice, PermissionChoice::Deny) {
            pending.record_tool_denied(&tool_call.name);
            let result = execute_tool_call(&mut pending, &request, &tool_call).await?;
            let mc = if result.model_content.is_empty() {
                None
            } else {
                Some(result.model_content)
            };
            self.state = RunState::Pending(Box::new(pending));
            return Ok(Some(UiEvent::ToolEnd {
                id: tool_call.id,
                status: result.status,
                summary: result.summary,
                model_content: mc,
                result: result.structured,
            }));
        }
        if requires_ordered_simple_execution(&tool_call.name)
            && self.has_active_ordered_simple_slot()
        {
            pending.queued_tool_calls.push_front(QueuedToolCall {
                request,
                tool_call,
                display_summary: queued_summary,
            });
            self.state = RunState::Pending(Box::new(pending));
            return Ok(None);
        }
        let hook_result = run_tool_pre_hooks(
            &mut pending,
            &tool_call.name,
            &tool_call.args,
            &tool_call.id,
        )
        .await?;
        if let Some(block) = hook_result.block {
            let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
            pending.record_tool_call(&tool_call.name);
            persist_blocked_tool_result(
                &pending.event_store,
                pending.execution_scope(),
                pending.turn,
                &tool_call.id,
                &block.reason,
            )?;
            self.state = RunState::Pending(Box::new(pending));
            return Ok(Some(UiEvent::ToolEnd {
                id: tool_call.id,
                status: "blocked".to_string(),
                summary: block.reason,
                model_content: None,
                result: Some(blocked),
            }));
        }
        let summary = display_summary(&tool_call.name, &hook_result.args, None);
        pending.record_tool_call(&tool_call.name);
        let (slot, tool_kind) =
            crate::query::slots::dispatch_tool_slot(crate::query::slots::SlotDispatchArgs {
                tool_name: tool_call.name.clone(),
                tool_id: tool_call.id.clone(),
                conversation: (!pending.conversation.is_main())
                    .then(|| pending.conversation.clone()),
                args: hook_result.args,
                summary: summary.clone(),
                workspace: pending.workspace.clone(),
                kuku_home: pending.kuku_home.clone(),
                prior_events: prior_events.clone(),
                event_tx: self.slot_event_tx.clone(),
                config: pending.config.clone(),
                catalog: pending.catalog.clone(),
                event_store: pending.event_store.clone(),
                parent_request: request,
                request_evidence_recorder: pending.request_evidence_recorder.clone(),
            });
        self.slots.insert(slot.tool_call_id.clone(), slot);
        self.state = RunState::Pending(Box::new(pending));
        Ok(Some(UiEvent::ToolStart {
            id: tool_call.id,
            tool: tool_call.name,
            summary,
            kind: tool_kind,
        }))
    }
}
