use std::sync::Arc;

use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::permission::append_project_allow_rule;
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::ProviderToolCall;
use crate::tool::ToolDefinition;

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

fn has_permission_decision(events: &[crate::event::StoredEvent], tool_call_id: &str) -> bool {
    events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::PermissionAllow { tool_call_id: id, .. }
                | EventPayload::PermissionDeny { tool_call_id: id, .. }
                if id == tool_call_id
        )
    })
}

fn persist_blocked_tool_result(
    events_path: &std::path::Path,
    turn: u64,
    tool_call_id: &str,
    summary: &str,
) -> Result<()> {
    let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
    let mut store = EventStore::open(events_path)?;
    store.append(EventPayload::ToolResult {
        turn,
        ts: now_timestamp()?,
        conversation: None,
        tool_call_id: tool_call_id.to_string(),
        status: "blocked".to_string(),
        summary: summary.to_string(),
        model_content: String::new(),
        truncated: false,
        files_read: Vec::new(),
        files_changed: Vec::new(),
        commands_run: Vec::new(),
        memory_changed: None,
        structured: Some(blocked),
    })?;
    Ok(())
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

pub(super) fn find_tool_definition<'a>(
    pending: &'a PendingRun,
    name: &str,
) -> Option<&'a ToolDefinition> {
    pending
        .resolved
        .as_ref()
        .and_then(|resolved| resolved.registry.iter().find(|tool| tool.name == name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventPayload, EventStore};
    use crate::provider::types::{ProviderKind, ProviderToolCall, ResolvedProvider, SecretString};
    use crate::query::types::{CumulativeUsage, ExecSlot, ResolvedRuntime, ToolKind};

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            tiers: std::collections::BTreeMap::new(),
            providers: std::collections::BTreeMap::new(),
            default_tier: "balanced".to_string(),
            discovery: crate::config::DiscoveryConfig::default(),
            handoff: crate::config::HandoffConfig::default(),
            logs: crate::config::LogsConfig::default(),
            plugin: crate::config::PluginConfig::default(),
            update: crate::config::UpdateConfig::default(),
        }
    }

    fn make_cancelled_run(events_path: std::path::PathBuf, turn: u64) -> Run {
        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        Run {
            session_id: "test".to_string(),
            state: RunState::Cancelled {
                events_path: events_path.clone(),
                turn,
            },
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        }
    }

    fn make_test_pending(
        events_path: std::path::PathBuf,
        dir: &std::path::Path,
        cancel_token: std::sync::Arc<tokio::sync::Notify>,
    ) -> PendingRun {
        PendingRun {
            session_id: "test".to_string(),
            query: crate::query::types::Query::new("test"),
            conversation: crate::conversation::address::ConversationAddress::MAIN,
            events_path,
            kuku_home: dir.to_path_buf(),
            workspace: dir.to_path_buf(),
            policy_path: dir.join("policy.md"),
            turn: 1,
            request_num: 1,
            cumulative: CumulativeUsage::default(),
            resolved: None,
            queued_tool_calls: std::collections::VecDeque::new(),
            resumed_permission_requests: std::collections::VecDeque::new(),
            config: std::sync::Arc::new(test_config()),
            prompts_dir: None,
            agent_registry: None,
            skill_registry: None,
            previous_skill_registry: None,
            bootstrap_skill: None,
            frozen_turn_prefix: crate::query::types::TurnPrefixFreeze::default(),
            child_session_count: 0,
            agent_binding_id: None,
            tool_registry_override: None,
            pending_events: std::collections::VecDeque::new(),
            pending_error: None,
            catalog: crate::prompt::builtin_prompt_catalog(),
            cancel_token,
            handoff_triggered: false,
            handoff_keep_turns: test_config().handoff().keep_turns,
            plugin_registry: None,
            hook_context: Vec::new(),
            force_continue_count: 0,
            model_request_count: 0,
            thinking_duration_ms: 0,
            tool_rounds: 0,
            tool_calls: 0,
            tool_names: Vec::new(),
            tool_denied: 0,
            tool_errors: 0,
            runtime_log_writer: crate::log::BufferedLogWriter::new(dir.join("runtime.jsonl")),
        }
    }

    fn make_waiting_run(
        events_path: std::path::PathBuf,
        dir: &std::path::Path,
        request_id: &str,
        request_tool_call_id: &str,
        queued_tool_call_id: &str,
    ) -> Run {
        let pending = make_test_pending(
            events_path,
            dir,
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        let mut pending = pending;
        pending.queued_tool_calls.push_back(QueuedToolCall {
            tool_call: ProviderToolCall {
                id: queued_tool_call_id.to_string(),
                name: "run_command".to_string(),
                args: serde_json::json!({"command": "printf hi", "timeout": 60, "brief": "print hi"}),
                index: 0,
            },
            display_summary: "print hi".to_string(),
        });
        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        Run {
            session_id: "test".to_string(),
            state: RunState::WaitingForPermission(Box::new(PendingPermission {
                pending,
                request: PermissionRequest {
                    id: request_id.to_string(),
                    conversation: crate::conversation::address::ConversationAddress::MAIN,
                    turn: 1,
                    tool_call_id: request_tool_call_id.to_string(),
                    tool: "run_command".to_string(),
                    risk: "command".to_string(),
                    summary: "print hi".to_string(),
                    candidate: "printf hi".to_string(),
                    source: "default_ask".to_string(),
                },
            })),
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        }
    }

    fn test_resolved_runtime() -> ResolvedRuntime {
        ResolvedRuntime {
            config: ResolvedProvider {
                kind: ProviderKind::OpenAiCompatible,
                model: "test-model".to_string(),
                base_url: "https://example.test".to_string(),
                api_key: SecretString::new("test-key"),
                max_context_tokens: 1000,
                max_output_tokens: 1000,
                think_level: crate::config::ThinkLevel::Off,
                thinking: crate::config::ResolvedThinking::default(),
            },
            registry: vec![ToolDefinition {
                name: "run_command".to_string(),
                description: "test command".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: false,
                max_result_chars: 8000,
                risk: "command".to_string(),
            }],
        }
    }

    fn make_queued_run(events_path: std::path::PathBuf, dir: &std::path::Path) -> Run {
        let mut pending = make_test_pending(
            events_path,
            dir,
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.resolved = Some(test_resolved_runtime());
        pending.queued_tool_calls.push_back(QueuedToolCall {
            tool_call: ProviderToolCall {
                id: "tool_queued".to_string(),
                name: "run_command".to_string(),
                args: serde_json::json!({"command": "printf hi", "timeout": 60, "brief": "print hi"}),
                index: 0,
            },
            display_summary: "print hi".to_string(),
        });
        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        Run {
            session_id: "test".to_string(),
            state: RunState::Pending(Box::new(pending)),
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        }
    }

    fn make_skill_registry() -> crate::skill::registry::SkillRegistry {
        let mut definition = crate::skill::definition::SkillDefinition {
            name: "review".to_string(),
            description: "Review code".to_string(),
            instructions: "Review carefully.".to_string(),
            source: crate::skill::definition::SkillSource::Project,
            hash: String::new(),
            source_path: Some("/skills/review".to_string()),
            allowed_tools: None,
            disallowed_tools: None,
            max_turns: None,
            model: None,
            license: None,
            compatibility: None,
            metadata: serde_json::Value::Null,
        };
        definition.hash = definition.compute_hash();
        crate::skill::registry::SkillRegistry::builder()
            .with_definition(definition)
            .build()
    }

    fn make_skill_queued_run(
        events_path: std::path::PathBuf,
        dir: &std::path::Path,
        registry: Vec<ToolDefinition>,
        tool_name: &str,
    ) -> Run {
        let mut pending = make_test_pending(
            events_path,
            dir,
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.resolved = Some(ResolvedRuntime {
            config: test_resolved_runtime().config,
            registry,
        });
        pending.skill_registry = Some(make_skill_registry());
        pending.queued_tool_calls.push_back(QueuedToolCall {
            tool_call: ProviderToolCall {
                id: "tool_skill".to_string(),
                name: tool_name.to_string(),
                args: serde_json::json!({"skill_name": "review", "query": "review"}),
                index: 0,
            },
            display_summary: "review".to_string(),
        });
        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        Run {
            session_id: "test".to_string(),
            state: RunState::Pending(Box::new(pending)),
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        }
    }

    fn assert_blocked_tool_result(events_path: &std::path::Path, summary: &str) {
        let events = EventStore::replay(events_path).unwrap();
        let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult {
                tool_call_id,
                status,
                summary: stored_summary,
                model_content,
                structured,
                ..
            } if tool_call_id == "tool_queued"
                && status == "blocked"
                && stored_summary == summary
                && model_content.is_empty()
                && structured.as_ref() == Some(&blocked)
        )));
    }

    fn write_blocking_pre_hook(pkg_dir: &std::path::Path, stderr_message: &str) {
        std::fs::create_dir_all(pkg_dir.join("hooks")).unwrap();

        #[cfg(windows)]
        let (command, hook_path, hook_body) = (
            "hooks/block.cmd",
            pkg_dir.join("hooks").join("block.cmd"),
            format!("@echo off\r\n<nul set /p ={stderr_message} 1>&2\r\nexit /b 2\r\n"),
        );

        #[cfg(not(windows))]
        let (command, hook_path, hook_body) = (
            "hooks/block.sh",
            pkg_dir.join("hooks").join("block.sh"),
            format!("#!/bin/sh\nprintf '{stderr_message}' >&2\nexit 2\n"),
        );

        std::fs::write(
            pkg_dir.join("kuku.toml"),
            format!(
                "[package]\nname = \"test-hook\"\nversion = \"1.0.0\"\n\n[[hooks]]\nevent = \"tool.pre_execute\"\ncommand = \"{command}\"\n",
            ),
        )
        .unwrap();
        std::fs::write(&hook_path, hook_body).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&hook_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&hook_path, permissions).unwrap();
        }
    }

    #[tokio::test]
    async fn queued_deny_persists_blocked_tool_result() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(
            dir.path().join("policy.md"),
            "# policy\n\n## allow\n\n## deny\n- run_command(printf hi)\n",
        )
        .unwrap();
        let mut run = make_queued_run(events_path.clone(), dir.path());

        let event = run.next().await.unwrap();

        assert!(
            matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
        );
        assert_blocked_tool_result(&events_path, "permission denied");
    }

    #[tokio::test]
    async fn queued_pre_hook_block_persists_blocked_tool_result() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(
            dir.path().join("policy.md"),
            "# policy\n\n## allow\n- run_command(printf hi)\n\n## deny\n",
        )
        .unwrap();
        let pkg_dir = dir.path().join(".kuku").join("packages").join("test-hook");
        write_blocking_pre_hook(&pkg_dir, "blocked by hook");
        let mut run = make_queued_run(events_path.clone(), dir.path());
        if let RunState::Pending(pending) = &mut run.state {
            pending.plugin_registry = Some(std::sync::Arc::new(
                crate::plugin::PluginRegistry::builder()
                    .load_packages(dir.path(), dir.path())
                    .unwrap()
                    .build()
                    .unwrap(),
            ));
        }

        let event = run.next().await.unwrap();

        assert!(
            matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
        );
        assert_blocked_tool_result(&events_path, "blocked by hook");
    }

    #[tokio::test]
    async fn inline_skill_tools_do_not_bypass_resolved_registry_membership() {
        let dir = tempfile::tempdir().unwrap();
        let registry = vec![ToolDefinition {
            name: "run_command".to_string(),
            description: "test command".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            read_only: false,
            max_result_chars: 8000,
            risk: "command".to_string(),
        }];

        for tool_name in ["use_skill", "list_skills", "search_skills"] {
            let events_path = dir.path().join(format!("{tool_name}.jsonl"));
            std::fs::write(&events_path, "").unwrap();
            let mut run =
                make_skill_queued_run(events_path, dir.path(), registry.clone(), tool_name);

            let event = run.next().await.unwrap();

            assert!(
                matches!(event, Some(UiEvent::Error { code, message }) if code == "unknown_tool" && message == format!("unknown tool: {tool_name}"))
            );
        }
    }

    #[tokio::test]
    async fn decide_pre_hook_block_persists_blocked_tool_result() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(dir.path().join("policy.md"), "# policy\n").unwrap();
        let pkg_dir = dir.path().join(".kuku").join("packages").join("test-hook");
        write_blocking_pre_hook(&pkg_dir, "blocked after allow");
        let mut run = make_queued_run(events_path.clone(), dir.path());
        let waiting = match std::mem::replace(&mut run.state, RunState::Done(None)) {
            RunState::Pending(mut pending) => {
                pending.plugin_registry = Some(std::sync::Arc::new(
                    crate::plugin::PluginRegistry::builder()
                        .load_packages(dir.path(), dir.path())
                        .unwrap()
                        .build()
                        .unwrap(),
                ));
                PendingPermission {
                    request: PermissionRequest {
                        id: "tool_queued".to_string(),
                        conversation: crate::conversation::address::ConversationAddress::MAIN,
                        turn: 1,
                        tool_call_id: "tool_queued".to_string(),
                        tool: "run_command".to_string(),
                        risk: "command".to_string(),
                        summary: "print hi".to_string(),
                        candidate: "printf hi".to_string(),
                        source: "default_ask".to_string(),
                    },
                    pending: *pending,
                }
            }
            other => panic!("expected pending run, got {other:?}"),
        };
        run.state = RunState::WaitingForPermission(Box::new(waiting));

        let event = run
            .decide("tool_queued", PermissionChoice::Once, None)
            .await
            .unwrap();

        assert!(
            matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
        );
        assert_blocked_tool_result(&events_path, "blocked after allow");
    }

    #[tokio::test]
    async fn cancel_when_idle_produces_turn_end() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        {
            let mut store = EventStore::open(&events_path).unwrap();
            store
                .append(EventPayload::SessionCreated {
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    schema_version: 2,
                    session_id: "test".to_string(),
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                    kuku_version: "0.1.0".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::TurnStarted {
                    turn: 1,
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    conversation: "main".to_string(),
                })
                .unwrap();
        }

        let mut run = make_cancelled_run(events_path.clone(), 1);
        let result = run.next().await.unwrap();
        assert!(matches!(result, Some(UiEvent::Cancelled { turn: 1 })));
        let result = run.next().await.unwrap();
        assert!(result.is_none());

        let events = EventStore::replay(&events_path).unwrap();
        let last = events.last().unwrap();
        assert!(matches!(
            &last.payload,
            EventPayload::TurnCancelled { turn: 1, .. }
        ));
    }

    #[tokio::test]
    async fn cancel_sets_token_and_transitions_state() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        let run = Run {
            session_id: "test".to_string(),
            state: RunState::Cancelled {
                events_path: events_path.clone(),
                turn: 1,
            },
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token: cancel_token.clone(),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        };

        cancel_token.notify_waiters();
        assert!(matches!(&run.state, RunState::Cancelled { .. }));
    }

    #[test]
    fn runtime_log_emit_fans_out_before_best_effort_persistence_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut pending = make_test_pending(
            dir.path().join("events.jsonl"),
            dir.path(),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.runtime_log_writer =
            crate::log::BufferedLogWriter::with_flush_every(dir.path().join("runtime.jsonl"), 1);
        pending.runtime_log_writer.set_fail_after_bytes(Some(0));

        let result = crate::query::provider::emit_runtime_log(
            &mut pending,
            crate::log::LogLevel::Info,
            "runtime.test",
            "test log",
            None,
        );

        assert!(result.is_ok());
        let Some(UiEvent::Log { record }) = pending.pending_events.pop_front() else {
            panic!("expected host-visible log event before persistence failure");
        };
        assert_eq!(record.kind, "runtime.test");
    }

    #[tokio::test]
    async fn runtime_log_persists_only_after_host_consumes_log_event() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let log_path = dir.path().join("runtime.jsonl");
        let mut pending = make_test_pending(
            events_path,
            dir.path(),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.runtime_log_writer = crate::log::BufferedLogWriter::with_flush_every(&log_path, 1);
        crate::query::provider::emit_runtime_log(
            &mut pending,
            crate::log::LogLevel::Warn,
            "runtime.warn",
            "warn log",
            None,
        )
        .unwrap();

        assert!(
            !log_path.exists(),
            "disk write happened before host delivery"
        );

        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        let mut run = Run {
            session_id: "test".to_string(),
            state: RunState::Pending(Box::new(pending)),
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        };

        let event = run.next().await.unwrap().expect("log event");
        assert!(matches!(event, UiEvent::Log { .. }));
        assert!(
            !log_path.exists(),
            "disk write happened before host consumed log"
        );

        let _ = run.next().await;
        assert!(
            log_path.exists(),
            "disk write should happen after host consumption"
        );
    }

    #[tokio::test]
    async fn completion_flush_failure_does_not_block_done() {
        let dir = tempfile::tempdir().unwrap();
        let mut pending = make_test_pending(
            dir.path().join("events.jsonl"),
            dir.path(),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.runtime_log_writer =
            crate::log::BufferedLogWriter::with_flush_every(dir.path().join("runtime.jsonl"), 64);
        pending.runtime_log_writer.set_fail_after_bytes(Some(0));
        crate::query::provider::emit_runtime_log(
            &mut pending,
            crate::log::LogLevel::Info,
            "runtime.test",
            "test log",
            None,
        )
        .unwrap();

        let state = StreamingChunkState {
            pending,
            conversation: crate::conversation::address::ConversationAddress::MAIN,
            request_id: "req_1".to_string(),
            stream: Box::pin(tokio_stream::empty()),
            accumulated_text: "complete".to_string(),
            accumulated_thinking: String::new(),
            stop_reason: Some("end_turn".to_string()),
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            provider_request_id: None,
            usage: None,
            lead_events: Vec::new(),
            handoff_detector: None,
            thinking_start: None,
            thinking_duration_ms: 0,
        };

        let step = crate::query::step::finish_streaming(state).await;

        assert!(matches!(step, Ok(PendingStep::Done(output, _, 1)) if output.text == "complete"));
    }

    #[tokio::test]
    async fn completion_persists_runtime_model_usage_log() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let log_path = dir.path().join("runtime.jsonl");
        let mut pending = make_test_pending(
            events_path,
            dir.path(),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.runtime_log_writer = crate::log::BufferedLogWriter::with_flush_every(&log_path, 1);

        let state = StreamingChunkState {
            pending,
            conversation: crate::conversation::address::ConversationAddress::MAIN,
            request_id: "req_7".to_string(),
            stream: Box::pin(tokio_stream::empty()),
            accumulated_text: "complete".to_string(),
            accumulated_thinking: String::new(),
            stop_reason: Some("end_turn".to_string()),
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            provider_request_id: None,
            usage: Some(crate::provider::types::ProviderUsage {
                input_tokens: Some(120),
                output_tokens: Some(30),
                cache_read_input_tokens: Some(900),
                cache_creation_input_tokens: Some(0),
            }),
            lead_events: Vec::new(),
            handoff_detector: None,
            thinking_start: None,
            thinking_duration_ms: 0,
        };

        let step = crate::query::step::finish_streaming(state).await;

        assert!(matches!(step, Ok(PendingStep::Done(output, _, 1)) if output.text == "complete"));
        let log = std::fs::read_to_string(&log_path).expect("runtime log should be written");
        assert!(log.contains("\"kind\":\"runtime.model_usage\""));
        assert!(log.contains("\"request_id\":\"req_7\""));
        assert!(log.contains("\"cache_read_input_tokens\":900"));
        assert!(log.contains("\"cache_hit_rate\":"));
    }

    #[tokio::test]
    async fn incomplete_handoff_marker_does_not_leak_to_final_output() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let mut pending = make_test_pending(
            events_path.clone(),
            dir.path(),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        pending.handoff_triggered = true;

        let stream: std::pin::Pin<
            Box<
                dyn futures_core::Stream<
                        Item = std::result::Result<
                            crate::provider::chunk::ProviderChunk,
                            crate::provider::types::ProviderFailure,
                        >,
                    > + Send,
            >,
        > = Box::pin(tokio_stream::iter(vec![
            Ok(crate::provider::chunk::ProviderChunk::TextDelta {
                text: "visible".to_string(),
            }),
            Ok(crate::provider::chunk::ProviderChunk::TextDelta {
                text: "\n\n<kuku_handoff".to_string(),
            }),
            Ok(crate::provider::chunk::ProviderChunk::StopReason {
                reason: "end_turn".to_string(),
            }),
            Ok(crate::provider::chunk::ProviderChunk::StreamEnd),
        ]));

        let mut streaming = StreamingChunkState {
            pending,
            conversation: crate::conversation::address::ConversationAddress::MAIN,
            request_id: "req_1".to_string(),
            stream,
            accumulated_text: String::new(),
            accumulated_thinking: String::new(),
            stop_reason: None,
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            provider_request_id: None,
            usage: None,
            lead_events: Vec::new(),
            handoff_detector: Some(crate::query::handoff::HandoffDetector::new()),
            thinking_start: None,
            thinking_duration_ms: 0,
        };
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

        loop {
            match Run::poll_stream_chunk(&cancel_token, &mut streaming)
                .await
                .unwrap()
            {
                Some(UiEvent::TextDelta { text }) => assert_eq!(text, "visible"),
                Some(_) => continue,
                None => break,
            }
        }
        let step = crate::query::step::finish_streaming(streaming)
            .await
            .unwrap();

        let PendingStep::Done(output, _, _) = step else {
            panic!("expected done step");
        };
        assert_eq!(output.text, "visible");

        let events = EventStore::replay(&events_path).unwrap();
        let response = events
            .iter()
            .find_map(|event| match &event.payload {
                EventPayload::ModelResponse { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .expect("model.response event");
        assert_eq!(response, "visible");
    }

    #[tokio::test]
    async fn cancel_during_streaming_aborts_stream() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, "").unwrap();
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

        let token_clone = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            token_clone.notify_waiters();
        });

        let pending = make_test_pending(events_path.clone(), dir.path(), cancel_token.clone());

        let stream: std::pin::Pin<
            Box<
                dyn futures_core::Stream<
                        Item = std::result::Result<
                            crate::provider::chunk::ProviderChunk,
                            crate::provider::types::ProviderFailure,
                        >,
                    > + Send
                    + Sync,
            >,
        > = Box::pin(tokio_stream::pending());

        let mut streaming = StreamingChunkState {
            pending,
            conversation: crate::conversation::address::ConversationAddress::MAIN,
            request_id: "req_1".to_string(),
            stream,
            accumulated_text: "partial".to_string(),
            accumulated_thinking: String::new(),
            stop_reason: None,
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            provider_request_id: None,
            usage: None,
            lead_events: Vec::new(),
            handoff_detector: None,
            thinking_start: None,
            thinking_duration_ms: 0,
        };

        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        let _run = Run {
            session_id: "test".to_string(),
            state: RunState::Pending(Box::new(make_test_pending(
                events_path.clone(),
                dir.path(),
                cancel_token.clone(),
            ))),
            slots: std::collections::HashMap::new(),
            slot_event_tx,
            slot_event_rx,
            cancel_token: cancel_token.clone(),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        };

        let result = Run::poll_stream_chunk(&cancel_token, &mut streaming)
            .await
            .unwrap();
        assert!(result.is_none());
        assert_eq!(streaming.stop_reason.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn malformed_tool_call_arguments_fail_instead_of_staying_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(&events_path, "").unwrap();
        let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

        let pending = make_test_pending(events_path, dir.path(), cancel_token.clone());
        let stream: std::pin::Pin<
            Box<
                dyn futures_core::Stream<
                        Item = std::result::Result<
                            crate::provider::chunk::ProviderChunk,
                            crate::provider::types::ProviderFailure,
                        >,
                    > + Send,
            >,
        > = Box::pin(tokio_stream::iter(vec![
            Ok(ProviderChunk::ToolCallStart {
                index: 0,
                id: "tool_bad_args".to_string(),
                name: "run_command".to_string(),
            }),
            Ok(ProviderChunk::ToolCallArgDelta {
                index: 0,
                fragment: "{\"command\":".to_string(),
            }),
            Ok(ProviderChunk::ContentBlockStop { index: 0 }),
            Ok(ProviderChunk::StopReason {
                reason: "tool_use".to_string(),
            }),
            Ok(ProviderChunk::StreamEnd),
        ]));

        let mut streaming = StreamingChunkState {
            pending,
            conversation: crate::conversation::address::ConversationAddress::MAIN,
            request_id: "req_bad_args".to_string(),
            stream,
            accumulated_text: String::new(),
            accumulated_thinking: String::new(),
            stop_reason: None,
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            provider_request_id: None,
            usage: None,
            lead_events: Vec::new(),
            handoff_detector: None,
            thinking_start: None,
            thinking_duration_ms: 0,
        };

        let error = loop {
            match Run::poll_stream_chunk(&cancel_token, &mut streaming).await {
                Ok(Some(_)) => continue,
                Ok(None) => panic!("expected malformed tool args to fail"),
                Err(error) => break error,
            }
        };

        assert!(matches!(
            error,
            Error::Provider { kind: crate::provider::types::ProviderFailureKind::InvalidRequest, message, .. }
                if message.contains("tool_bad_args")
        ));
    }

    #[tokio::test]
    async fn cancelled_tool_result_envelope_has_correct_fields() {
        let result = crate::tool::ToolResultEnvelope::cancelled("test cancel");
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.summary, "test cancel");
        assert!(result.model_content.is_empty());
        assert!(!result.truncated);
        assert_eq!(
            result.structured,
            Some(serde_json::json!({"kind": "cancelled"}))
        );
    }

    #[test]
    fn cancel_pending_permission_rejects_mismatched_queued_tool() {
        let dir = tempfile::tempdir().unwrap();
        let mut run = make_waiting_run(
            dir.path().join("events.jsonl"),
            dir.path(),
            "req_cancel",
            "tool_request",
            "tool_queued",
        );

        let error = run.cancel_pending_permission("req_cancel").unwrap_err();

        assert!(
            matches!(error, Error::InvalidEventStream(message) if message.contains("tool_request") && message.contains("tool_queued"))
        );
        assert!(matches!(
            &run.state,
            RunState::WaitingForPermission(waiting)
                if waiting.request.tool_call_id == "tool_request"
                    && waiting.pending.queued_tool_calls.front().unwrap().tool_call.id == "tool_queued"
        ));
    }

    #[test]
    fn cancel_pending_permission_restores_state_when_persistence_fails() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events_dir");
        std::fs::create_dir(&events_path).unwrap();
        let mut run = make_waiting_run(
            events_path,
            dir.path(),
            "req_cancel",
            "tool_cancel",
            "tool_cancel",
        );

        let error = run.cancel_pending_permission("req_cancel").unwrap_err();

        assert!(matches!(error, Error::Io(_)));
        assert!(matches!(
            &run.state,
            RunState::WaitingForPermission(waiting)
                if waiting.request.id == "req_cancel"
                    && waiting.pending.queued_tool_calls.front().unwrap().tool_call.id == "tool_cancel"
        ));
    }

    #[tokio::test]
    async fn cancel_waiting_permission_writes_cancelled_result_without_deny() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let mut run = make_waiting_run(
            events_path.clone(),
            dir.path(),
            "req_cancel",
            "tool_cancel",
            "tool_cancel",
        );

        run.cancel();
        let event = run.next().await.unwrap();

        assert!(matches!(event, Some(UiEvent::Cancelled { turn: 1 })));
        let events = EventStore::replay(&events_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event.payload,
            EventPayload::ToolResult { ref tool_call_id, ref status, ref structured, .. }
                if tool_call_id == "tool_cancel"
                    && status == "cancelled"
                    && structured == &Some(serde_json::json!({"kind": "cancelled"}))
        )));
        assert!(!events.iter().any(|event| matches!(
            event.payload,
            EventPayload::PermissionDeny { ref tool_call_id, .. } if tool_call_id == "tool_cancel"
        )));
    }

    #[tokio::test]
    async fn cancelled_run_persists_tool_result_for_finished_active_slot() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let mut store = EventStore::open(&events_path).unwrap();
        store
            .append(EventPayload::TurnStarted {
                turn: 1,
                ts: "2026-05-20T00:00:00Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
        store
            .append(EventPayload::ToolCall {
                turn: 1,
                ts: "2026-05-20T00:00:01Z".to_string(),
                conversation: None,
                tool_call_id: "tool_cancelled".to_string(),
                request_id: "req_1".to_string(),
                index: 0,
                tool: "run_command".to_string(),
                args: serde_json::json!({"command": "printf hi", "timeout": 60, "brief": "print hi"}),
            })
            .unwrap();

        let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            "tool_cancelled".to_string(),
            ExecSlot {
                tool_call_id: "tool_cancelled".to_string(),
                conversation: None,
                kind: ToolKind::Command { pid: None },
                ordered_with_simple_tools: false,
                label: "print hi".to_string(),
                cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
                nested_permissions: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
            },
        );
        let mut run = Run {
            session_id: "test".to_string(),
            state: RunState::Cancelled {
                events_path: events_path.clone(),
                turn: 1,
            },
            slots,
            slot_event_tx: slot_event_tx.clone(),
            slot_event_rx,
            cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
            lock_path: std::path::PathBuf::new(),
            deferred_runtime_logs: std::collections::VecDeque::new(),
        };

        slot_event_tx
            .send((
                "tool_cancelled".to_string(),
                SlotEvent::Done {
                    status: "ok".to_string(),
                    summary: "finished after cancellation".to_string(),
                    model_content: String::new(),
                    result: Some(serde_json::json!({"kind": "command_result"})),
                },
            ))
            .await
            .unwrap();

        let event = run.next().await.unwrap();

        assert!(matches!(
            event,
            Some(UiEvent::ToolEnd { ref id, ref status, .. })
                if id == "tool_cancelled" && status == "ok"
        ));
        let events = EventStore::replay(&events_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ToolResult { tool_call_id, status, summary, .. }
                if tool_call_id == "tool_cancelled"
                    && status == "ok"
                    && summary == "finished after cancellation"
        )));
    }

    #[tokio::test]
    async fn resume_after_cancel_includes_turn_end_in_history() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        {
            let mut store = EventStore::open(&events_path).unwrap();
            store
                .append(EventPayload::SessionCreated {
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    schema_version: 2,
                    session_id: "test".to_string(),
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                    kuku_version: "0.1.0".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::TurnStarted {
                    turn: 1,
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    conversation: "main".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::MessageUser {
                    turn: 1,
                    ts: "2026-05-20T00:00:01Z".to_string(),
                    conversation: "main".to_string(),
                    text: "hello".to_string(),
                    from: None,
                    via_tool_call_id: None,
                })
                .unwrap();
            store
                .append(EventPayload::ModelResponse {
                    turn: 1,
                    ts: "2026-05-20T00:00:02Z".to_string(),
                    request_id: "req_1".to_string(),
                    text: "partial".to_string(),
                    thinking: None,
                    input_tokens_total: None,
                })
                .unwrap();
            store
                .append(EventPayload::TurnCompleted {
                    turn: 1,
                    ts: "2026-05-20T00:00:03Z".to_string(),
                    conversation: "main".to_string(),
                })
                .unwrap();
        }

        let events = EventStore::replay(&events_path).unwrap();
        let (summary, history) = crate::context::rebuild_history(
            &events,
            &crate::conversation::address::ConversationAddress::MAIN,
        );
        assert!(summary.is_none());
        assert_eq!(history.len(), 2);
        let messages: Vec<_> = history.iter().map(|m| format!("{:?}", m.role)).collect();
        assert!(messages.contains(&"User".to_string()));
        assert!(messages.contains(&"Assistant".to_string()));
    }
}
