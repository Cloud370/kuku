use std::sync::Arc;
mod helpers;
#[cfg(test)]
mod tests;
use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::permission::append_project_allow_rule;
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::ProviderToolCall;
use helpers::{has_permission_decision, persist_blocked_tool_result};

use super::helpers::{
    append_model_error, append_permission_decision, append_permission_request,
    append_turn_cancelled, append_turn_interrupted, display_summary, is_inline_skill_tool,
    now_timestamp, permission_candidate, permission_rule, resolved_tool_available,
};
use super::slots::requires_ordered_simple_execution;
use super::tool_exec::{execute_tool_call, run_tool_pre_hooks};
use super::types::{
    PendingPermission, PendingRun, PendingStep, PermissionChoice, PermissionRequest,
    QueuedToolCall, Run, RunState, SlotEvent, StreamingChunkState, UiEvent,
};

impl Drop for Run {
    fn drop(&mut self) {
        crate::session::release_lock(&self.lock_path);
    }
}

impl Run {
    fn has_active_ordered_simple_slot(&self) -> bool {
        self.slots
            .values()
            .any(|slot| slot.ordered_with_simple_tools)
    }

    /// The session ID for this run.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The workspace directory for this run.
    pub fn workspace(&self) -> &std::path::Path {
        match &self.state {
            RunState::Pending(p) => &p.workspace,
            RunState::Streaming(s) => &s.pending.workspace,
            RunState::WaitingForPermission(w) => &w.pending.workspace,
            RunState::Cancelled { .. } | RunState::Done(_) => std::path::Path::new(""),
        }
    }

    /// A token that is notified when the run is cancelled.
    pub fn cancel_token(&self) -> Arc<tokio::sync::Notify> {
        self.cancel_token.clone()
    }

    /// Cancel the current run. Streaming is aborted, pending permissions are denied,
    /// and the cancelled model.response enters history.
    pub fn cancel(&mut self) {
        for slot in self.slots.values() {
            slot.cancel.notify_one();
        }
        let (events_path, turn) = match std::mem::replace(&mut self.state, RunState::Done(None)) {
            RunState::Pending(mut pending) => {
                self.persist_deferred_runtime_logs_for_pending(&mut pending);
                pending.flush_runtime_logs();
                (pending.events_path.clone(), pending.turn)
            }
            RunState::Streaming(mut streaming) => {
                self.persist_deferred_runtime_logs_for_pending(&mut streaming.pending);
                streaming.pending.flush_runtime_logs();
                (
                    streaming.pending.events_path.clone(),
                    streaming.pending.turn,
                )
            }
            RunState::WaitingForPermission(mut waiting) => {
                self.persist_deferred_runtime_logs_for_pending(&mut waiting.pending);
                waiting.pending.flush_runtime_logs();
                if self
                    .close_pending_permission_as_cancelled(&waiting)
                    .is_err()
                {
                    self.state = RunState::WaitingForPermission(waiting);
                    self.cancel_token.notify_waiters();
                    return;
                }
                (waiting.pending.events_path.clone(), waiting.pending.turn)
            }
            other @ (RunState::Cancelled { .. } | RunState::Done(_)) => {
                self.state = other;
                return;
            }
        };
        self.state = RunState::Cancelled { events_path, turn };
        self.cancel_token.notify_waiters();
    }

    /// Poll for the next UI event from the running query.
    pub async fn next(&mut self) -> Result<Option<UiEvent>> {
        loop {
            self.persist_deferred_runtime_logs();

            // 1. Permission queue priority — don't wait for slots
            if matches!(&self.state, RunState::Pending(_)) {
                if let Some(event) = self.try_process_queued_call().await? {
                    return Ok(Some(self.defer_runtime_log_if_needed(event)));
                }
            }

            // 2. Poll running slots via shared channel
            if !self.slots.is_empty() {
                let slot_event = tokio::select! {
                    event = self.slot_event_rx.recv() => event,
                    _ = self.cancel_token.notified() => None,
                };
                if let Some((tool_call_id, event)) = slot_event {
                    match event {
                        SlotEvent::Output(te) => {
                            return Ok(Some(UiEvent::ToolOutput {
                                id: tool_call_id,
                                event: te,
                            }));
                        }
                        SlotEvent::Done {
                            status,
                            summary,
                            model_content,
                            result,
                        } => {
                            let slot = self.slots.remove(&tool_call_id).expect("slot must exist");
                            let (events_path, turn) = match &self.state {
                                RunState::Pending(p) => (&p.events_path, p.turn),
                                RunState::Streaming(s) => (&s.pending.events_path, s.pending.turn),
                                RunState::WaitingForPermission(w) => {
                                    (&w.pending.events_path, w.pending.turn)
                                }
                                RunState::Cancelled { events_path, turn } => (events_path, *turn),
                                _ => {
                                    return Ok(Some(UiEvent::ToolEnd {
                                        id: slot.tool_call_id,
                                        status,
                                        summary,
                                        model_content: None,
                                        result,
                                    }));
                                }
                            };
                            let result = super::tool_exec::write_tool_result(
                                &slot,
                                &status,
                                &summary,
                                &model_content,
                                &result,
                                events_path,
                                turn,
                            )?;
                            let mc = if model_content.is_empty() {
                                None
                            } else {
                                Some(model_content)
                            };
                            return Ok(Some(UiEvent::ToolEnd {
                                id: slot.tool_call_id,
                                status,
                                summary,
                                model_content: mc,
                                result,
                            }));
                        }
                    }
                }
            }

            match std::mem::replace(&mut self.state, RunState::Done(None)) {
                RunState::Pending(pending) => {
                    if let Some(event) = self.advance_from_pending(pending).await? {
                        return Ok(Some(self.defer_runtime_log_if_needed(event)));
                    }
                }
                RunState::Streaming(streaming) => {
                    if let Some(event) = self.advance_from_streaming(streaming).await? {
                        return Ok(Some(self.defer_runtime_log_if_needed(event)));
                    }
                }
                RunState::WaitingForPermission(waiting) => {
                    let request = waiting.request.clone();
                    self.state = RunState::WaitingForPermission(waiting);
                    return Ok(Some(UiEvent::PermissionRequested { request }));
                }
                RunState::Cancelled { events_path, turn } => {
                    append_turn_cancelled(
                        &events_path,
                        &crate::conversation::address::ConversationAddress::MAIN,
                        turn,
                        "user_cancelled",
                    )?;
                    self.state = RunState::Done(None);
                    return Ok(Some(UiEvent::Cancelled { turn }));
                }
                RunState::Done(Some((output, usage, turn))) => {
                    self.state = RunState::Done(None);
                    return Ok(Some(UiEvent::Done {
                        output,
                        usage,
                        turn,
                    }));
                }
                RunState::Done(None) => return Ok(None),
            }
        }
    }

    fn defer_runtime_log_if_needed(&mut self, event: UiEvent) -> UiEvent {
        if let UiEvent::Log { record } = &event {
            self.deferred_runtime_logs.push_back(record.clone());
        }
        event
    }

    fn persist_deferred_runtime_logs(&mut self) {
        let Some(record) = self.deferred_runtime_logs.pop_front() else {
            return;
        };
        match &mut self.state {
            RunState::Pending(pending) => {
                let _ = pending.runtime_log_writer.push(record);
            }
            RunState::Streaming(streaming) => {
                let _ = streaming.pending.runtime_log_writer.push(record);
            }
            RunState::WaitingForPermission(waiting) => {
                let _ = waiting.pending.runtime_log_writer.push(record);
            }
            RunState::Cancelled { .. } | RunState::Done(_) => {}
        }
    }

    fn persist_deferred_runtime_logs_for_pending(&mut self, pending: &mut PendingRun) {
        while let Some(record) = self.deferred_runtime_logs.pop_front() {
            let _ = pending.runtime_log_writer.push(record);
        }
    }

    async fn advance_from_pending(
        &mut self,
        pending: Box<super::types::PendingRun>,
    ) -> Result<Option<UiEvent>> {
        match super::step::advance_pending(*pending, self.slot_event_tx.clone(), self.slots.len())
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

    async fn advance_from_streaming(
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
                record_streaming_provider_error_facts(&streaming, &error);
                streaming.pending.flush_runtime_logs();
                Err(error)
            }
            Ok(Some(event)) => {
                self.state = RunState::Streaming(streaming);
                Ok(Some(event))
            }
            Ok(None) => {
                self.persist_deferred_runtime_logs_for_pending(&mut streaming.pending);
                let step = super::step::finish_streaming(*streaming).await?;
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

    async fn try_process_queued_call(&mut self) -> Result<Option<UiEvent>> {
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
        super::provider::ensure_resolved(pending)?;
        let queued = match pending.queued_tool_calls.front() {
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
                    let choice = super::helpers::gate_choice(&decision.source);
                    if !has_permission_decision(&prior_events, &queued.tool_call.id) {
                        append_permission_decision(
                            &pending.events_path,
                            pending.turn,
                            &queued.tool_call.id,
                            choice,
                            super::helpers::gate_source_name(decision.source),
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
                let (slot, tool_kind) =
                    super::slots::dispatch_tool_slot(super::slots::SlotDispatchArgs {
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
                    });
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
                        source: super::helpers::gate_source_name(decision.source).to_string(),
                    },
                )?;
                append_permission_decision(
                    &pending.events_path,
                    pending.turn,
                    &tool_call.id,
                    PermissionChoice::Deny,
                    super::helpers::gate_source_name(decision.source),
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

    async fn poll_stream_chunk(
        cancel_token: &tokio::sync::Notify,
        streaming: &mut StreamingChunkState,
    ) -> Result<Option<UiEvent>> {
        use tokio_stream::StreamExt;
        loop {
            let chunk = tokio::select! {
                chunk = streaming.stream.next() => match chunk {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(failure)) => {
                        return Err(crate::error::Error::Provider {
                            kind: failure.kind,
                            message: failure.message,
                            provider: None,
                            model: None,
                        });
                    }
                    None => return Ok(None),
                },
                _ = cancel_token.notified() => {
                    streaming.stop_reason = Some("cancelled".to_string());
                    return Ok(None);
                }
            };

            match chunk {
                ProviderChunk::StreamStart { request_id: rid } => {
                    streaming.provider_request_id = Some(rid);
                }
                ProviderChunk::TextDelta { text } => {
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    if let Some(ref mut detector) = streaming.handoff_detector {
                        if let Some(user_text) = detector.process(&text) {
                            if !user_text.is_empty() {
                                streaming.accumulated_text.push_str(&user_text);
                                return Ok(Some(UiEvent::TextDelta { text: user_text }));
                            }
                        }
                        return Ok(None);
                    }
                    streaming.accumulated_text.push_str(&text);
                    return Ok(Some(UiEvent::TextDelta { text }));
                }
                ProviderChunk::ThinkingDelta { text } => {
                    if streaming.thinking_start.is_none() {
                        streaming.thinking_start = Some(std::time::Instant::now());
                    }
                    streaming.accumulated_thinking.push_str(&text);
                    return Ok(Some(UiEvent::ThinkingDelta { text }));
                }
                ProviderChunk::ToolCallStart { index, id, name } => {
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    streaming.tool_calls.push(ProviderToolCall {
                        id,
                        name,
                        args: serde_json::json!({}),
                        index,
                    });
                    streaming.tool_arg_buffers.push((index, String::new()));
                }
                ProviderChunk::ToolCallArgDelta { index, fragment } => {
                    if let Some((_, buf)) = streaming
                        .tool_arg_buffers
                        .iter_mut()
                        .find(|(i, _)| *i == index)
                    {
                        buf.push_str(&fragment);
                    }
                }
                ProviderChunk::ContentBlockStop { index } => {
                    if let Some((_, buf)) =
                        streaming.tool_arg_buffers.iter().find(|(i, _)| *i == index)
                    {
                        match serde_json::from_str::<serde_json::Value>(buf) {
                            Ok(args) => {
                                if let Some(tc) =
                                    streaming.tool_calls.iter_mut().find(|t| t.index == index)
                                {
                                    tc.args = args;
                                }
                            }
                            Err(error) => {
                                let tool_call_id = streaming
                                    .tool_calls
                                    .iter()
                                    .find(|t| t.index == index)
                                    .map(|tool_call| tool_call.id.clone())
                                    .unwrap_or_else(|| format!("index {index}"));
                                return Err(crate::error::Error::Provider {
                                    kind: crate::provider::types::ProviderFailureKind::InvalidRequest,
                                    message: format!(
                                        "tool call {tool_call_id} has invalid JSON arguments: {error}"
                                    ),
                                    provider: None,
                                    model: None,
                                });
                            }
                        }
                    }
                }
                ProviderChunk::StopReason { reason } => {
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    streaming.stop_reason = Some(reason);
                }
                ProviderChunk::StreamUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                } => {
                    let entry =
                        streaming
                            .usage
                            .get_or_insert(crate::provider::types::ProviderUsage {
                                input_tokens: Some(0),
                                output_tokens: Some(0),
                                cache_read_input_tokens: Some(0),
                                cache_creation_input_tokens: Some(0),
                            });
                    entry.input_tokens = Some(entry.input_tokens.unwrap_or(0) + input_tokens);
                    entry.output_tokens = Some(entry.output_tokens.unwrap_or(0) + output_tokens);
                    entry.cache_read_input_tokens =
                        Some(entry.cache_read_input_tokens.unwrap_or(0) + cache_read_input_tokens);
                    entry.cache_creation_input_tokens = Some(
                        entry.cache_creation_input_tokens.unwrap_or(0)
                            + cache_creation_input_tokens,
                    );
                }
                ProviderChunk::ServerError { code, message } => {
                    return Err(crate::error::Error::Provider {
                        kind: crate::provider::types::ProviderFailureKind::Unknown,
                        message: format!("{code}: {message}"),
                        provider: None,
                        model: None,
                    });
                }
                ProviderChunk::StreamEnd => {}
            }
        }
    }

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

    pub(super) async fn deny_pending(&mut self) -> Result<Option<UiEvent>> {
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

    fn close_pending_permission_as_cancelled(
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
        let mut store = EventStore::open(&waiting.pending.events_path)?;
        store.append(EventPayload::ToolResult {
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
            &pending.events_path,
            pending.turn,
            &tool_call.id,
            choice,
            source,
            &rule,
        )?;
        let prior_events = EventStore::replay(&pending.events_path)?;
        if matches!(choice, PermissionChoice::Deny) {
            pending.record_tool_denied(&tool_call.name);
            let result = execute_tool_call(&mut pending, &tool_call).await?;
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
                &pending.events_path,
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
        let (slot, tool_kind) = super::slots::dispatch_tool_slot(super::slots::SlotDispatchArgs {
            tool_name: tool_call.name.clone(),
            tool_id: tool_call.id.clone(),
            conversation: (!pending.conversation.is_main()).then(|| pending.conversation.clone()),
            args: hook_result.args,
            summary: summary.clone(),
            workspace: pending.workspace.clone(),
            kuku_home: pending.kuku_home.clone(),
            prior_events: prior_events.clone(),
            event_tx: self.slot_event_tx.clone(),
            config: pending.config.clone(),
            catalog: pending.catalog.clone(),
            events_path: pending.events_path.clone(),
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

fn record_streaming_provider_error_facts(streaming: &StreamingChunkState, error: &Error) {
    let Error::Provider { kind, message, .. } = error else {
        return;
    };
    let _ = append_model_error(
        &streaming.pending.events_path,
        streaming.pending.turn,
        streaming.request_id.clone(),
        provider_failure_event_kind(*kind),
        message,
    );
    let _ = append_turn_interrupted(
        &streaming.pending.events_path,
        &streaming.conversation,
        streaming.pending.turn,
        provider_failure_event_kind(*kind),
    );
}

fn provider_failure_event_kind(kind: crate::provider::types::ProviderFailureKind) -> &'static str {
    match kind {
        crate::provider::types::ProviderFailureKind::Authentication => "authentication",
        crate::provider::types::ProviderFailureKind::RateLimited => "rate_limited",
        crate::provider::types::ProviderFailureKind::ContextTooLarge => "context_too_large",
        crate::provider::types::ProviderFailureKind::InvalidRequest => "invalid_request",
        crate::provider::types::ProviderFailureKind::ProviderUnavailable => "provider_unavailable",
        crate::provider::types::ProviderFailureKind::Transport => "transport",
        crate::provider::types::ProviderFailureKind::Internal => "internal",
        crate::provider::types::ProviderFailureKind::Unknown => "unknown",
    }
}

async fn run_session_end_hooks(output: &super::types::RunOutput, turn: u64) {
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
        let _ = super::tool_exec::record_plugin_hooks(
            &output.session_dir,
            turn,
            "session.end",
            &results,
        );
    }
}

pub(crate) fn find_tool_definition<'a>(
    pending: &'a PendingRun,
    name: &str,
) -> Option<&'a crate::tool::ToolDefinition> {
    helpers::find_tool_definition(pending, name)
}
