use super::{
    append_permission_decision, append_permission_request, display_summary, find_tool_definition,
    has_permission_decision, is_inline_skill_tool, permission_candidate, permission_rule,
    persist_blocked_tool_result, requires_ordered_simple_execution, resolved_tool_available,
    run_tool_pre_hooks, PendingPermission, PendingStep, PermissionChoice, PermissionRequest,
    QueuedToolCall, Result, Run, RunState, StreamingChunkState, UiEvent,
};

impl Run {
    pub(super) async fn advance_from_pending(
        &mut self,
        pending: Box<crate::query::types::PendingRun>,
    ) -> Result<Option<UiEvent>> {
        match crate::query::step::advance_pending(
            *pending,
            self.slot_event_tx.clone(),
            self.slots.len(),
        )
        .await?
        {
            PendingStep::Pending {
                pending,
                slot,
                event,
            } => {
                if let Some(slot) = slot {
                    self.slots.insert(slot.tool_call_id.clone(), slot);
                }
                self.state = RunState::Pending(pending);
                Ok(event)
            }
            PendingStep::NeedPermission(waiting) => {
                let request = waiting.request.clone();
                self.state = RunState::WaitingForPermission(waiting);
                Ok(Some(UiEvent::PermissionRequested { request }))
            }
            PendingStep::Streaming(streaming) => {
                self.state = RunState::Streaming(streaming);
                Ok(None)
            }
            PendingStep::Done(output, usage, turn) => {
                run_session_end_hooks(&output, turn).await;
                self.state = RunState::Done(None);
                Ok(Some(UiEvent::Done {
                    output,
                    usage,
                    turn,
                }))
            }
            PendingStep::Failed(error) => {
                self.state = RunState::Done(None);
                Err(error)
            }
        }
    }

    pub(super) async fn advance_from_streaming(
        &mut self,
        mut streaming: Box<StreamingChunkState>,
    ) -> Result<Option<UiEvent>> {
        if let Some(event) = streaming.lead_events.pop() {
            self.state = RunState::Streaming(streaming);
            return Ok(Some(event));
        }
        let poll = Self::poll_stream_chunk(&self.cancel_token, &mut streaming).await;
        match poll {
            Err(error) => {
                self.persist_deferred_runtime_logs_for_pending(&mut streaming.pending);
                super::stream::record_streaming_provider_error_facts(&streaming, &error)?;
                streaming.pending.flush_runtime_logs();
                Err(error)
            }
            Ok(Some(event)) => {
                self.state = RunState::Streaming(streaming);
                Ok(Some(event))
            }
            Ok(None) => {
                self.persist_deferred_runtime_logs_for_pending(&mut streaming.pending);
                let step = crate::query::step::finish_streaming(*streaming).await?;
                match step {
                    PendingStep::Pending { pending, .. } => {
                        self.state = RunState::Pending(pending);
                        Ok(None)
                    }
                    PendingStep::Done(output, usage, turn) => {
                        run_session_end_hooks(&output, turn).await;
                        self.state = RunState::Done(None);
                        Ok(Some(UiEvent::Done {
                            output,
                            usage,
                            turn,
                        }))
                    }
                    _ => {
                        self.state = RunState::Done(None);
                        Ok(None)
                    }
                }
            }
        }
    }

    pub(super) async fn try_process_queued_call(&mut self) -> Result<Option<UiEvent>> {
        let has_active_ordered_simple_slot = self.has_active_ordered_simple_slot();
        let (front_tool_call_id, front_tool_name) = match &self.state {
            RunState::Pending(pending) => match pending.queued_tool_calls.front() {
                Some(queued) => (queued.tool_call.id.clone(), queued.tool_call.name.clone()),
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        let resumed_request = match &mut self.state {
            RunState::Pending(pending) => {
                pending.take_resumed_permission_request(&front_tool_call_id)
            }
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
        if front_tool_name == "agent"
            || (is_inline_skill_tool(&front_tool_name)
                && resolved_tool_available(pending, &front_tool_name))
        {
            return Ok(None);
        }
        if requires_ordered_simple_execution(&front_tool_name) && has_active_ordered_simple_slot {
            return Ok(None);
        }
        crate::query::provider::ensure_resolved(pending)?;
        let queued = match pending.queued_tool_calls.front() {
            Some(q) => q,
            None => return Ok(None),
        };

        let policy = crate::permission::load_project_policy(&pending.policy_path)?;
        let prior_events = pending.event_store.read_all()?;
        let session_grants = crate::permission::recover_session_grants(&prior_events);

        let definition = match find_tool_definition(pending, &queued.tool_call.name) {
            Some(d) => d,
            None => {
                let QueuedToolCall { tool_call, .. } =
                    pending.queued_tool_calls.pop_front().unwrap();
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
                            &pending.event_store,
                            pending.execution_scope(),
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
                    request,
                    tool_call,
                    display_summary,
                } = pending.queued_tool_calls.pop_front().unwrap();
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
                        &pending.event_store,
                        pending.execution_scope(),
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
                        event_store: pending.event_store.clone(),
                        parent_request: request,
                        request_evidence_recorder: pending.request_evidence_recorder.clone(),
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
                    pending.queued_tool_calls.pop_front().unwrap();
                append_permission_request(
                    &pending.event_store,
                    pending.execution_scope(),
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
                    &pending.event_store,
                    pending.execution_scope(),
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
                    &pending.event_store,
                    pending.execution_scope(),
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

async fn run_session_end_hooks(output: &crate::query::types::RunOutput, turn: u64) {
    let Some(ref plugin_reg) = output.plugin_registry else {
        return;
    };
    let hooks = plugin_reg.hooks_for(crate::plugin::hook::HookEvent::SessionEnd);
    if hooks.is_empty() {
        return;
    }
    let input = crate::plugin::executor::HookInput {
        event: "session.end".to_string(),
        session_dir: output.session_dir.to_string_lossy().to_string(),
        extra: serde_json::json!({}),
    };
    if let Ok(results) = crate::plugin::executor::execute_hooks(
        hooks,
        &input,
        &output.session_dir,
        &output.workspace,
    )
    .await
    {
        let _ = crate::query::tool_exec::record_plugin_hooks(
            &output.session_dir,
            turn,
            "session.end",
            &results,
        );
    }
}
