use std::sync::Arc;

use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::permission::append_project_allow_rule;
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::ProviderToolCall;
use crate::tool::ToolDefinition;

use super::helpers::{
    append_model_error, append_permission_decision, append_permission_request, append_turn_end,
    display_summary, now_timestamp, permission_candidate, permission_rule,
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
                    let mut store = EventStore::open(&events_path)?;
                    store.append(EventPayload::TurnEnd {
                        turn,
                        ts: now_timestamp()?,
                    })?;
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

        if front_tool_name == "agent" || front_tool_name == "use_skill" {
            return Ok(None);
        }
        if requires_ordered_simple_execution(&front_tool_name) && has_active_ordered_simple_slot {
            return Ok(None);
        }

        let pending = match &mut self.state {
            RunState::Pending(p) => p.as_mut(),
            _ => return Ok(None),
        };
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
                    pending.record_tool_call(&tool_call.name);
                    return Ok(Some(UiEvent::ToolEnd {
                        id: tool_call.id,
                        status: "blocked".to_string(),
                        summary: block.reason,
                        model_content: None,
                        result: None,
                    }));
                }
                pending.record_tool_call(&tool_call.name);
                let (slot, tool_kind) =
                    super::slots::dispatch_tool_slot(super::slots::SlotDispatchArgs {
                        tool_name: tool_call.name.clone(),
                        tool_id: tool_call.id.clone(),
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
                    pending.turn,
                    &PermissionRequest {
                        id: tool_call.id.clone(),
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
                Ok(Some(UiEvent::ToolEnd {
                    id: tool_call.id,
                    status: "blocked".to_string(),
                    summary: "permission denied".to_string(),
                    model_content: None,
                    result: None,
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
                    streaming.accumulated_text.push_str(&text);
                    if let Some(ref mut detector) = streaming.handoff_detector {
                        if let Some(user_text) = detector.process(&text) {
                            if !user_text.is_empty() {
                                return Ok(Some(UiEvent::TextDelta { text: user_text }));
                            }
                        }
                        return Ok(None);
                    }
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
                        if let Ok(args) = serde_json::from_str::<serde_json::Value>(buf) {
                            if let Some(tc) =
                                streaming.tool_calls.iter_mut().find(|t| t.index == index)
                            {
                                tc.args = args;
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
    /// `parent_tool_id`: `None` for top-level, `Some(id)` for subagent permission.
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
            let mut map = slot.child_permissions.lock().unwrap();
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
                ))
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

        let tool_call = match waiting.pending.queued_tool_calls.front() {
            Some(queued) if queued.tool_call.id == waiting.request.tool_call_id => {
                queued.tool_call.clone()
            }
            Some(queued) => {
                let message = format!(
                    "pending permission {} expects tool call {}, but queued tool call is {}",
                    waiting.request.id, waiting.request.tool_call_id, queued.tool_call.id
                );
                self.state = RunState::WaitingForPermission(Box::new(waiting));
                return Err(Error::InvalidEventStream(message));
            }
            None => {
                self.state = RunState::WaitingForPermission(Box::new(waiting));
                return Err(Error::InvalidEventStream(format!(
                    "pending permission {} has no queued tool call",
                    request_id
                )));
            }
        };
        let result = crate::tool::ToolResultEnvelope::cancelled("permission request cancelled");
        let write_result = (|| -> Result<()> {
            let mut store = EventStore::open(&waiting.pending.events_path)?;
            store.append(EventPayload::ToolResult {
                turn: waiting.pending.turn,
                ts: now_timestamp()?,
                tool_call_id: tool_call.id.clone(),
                status: result.status.clone(),
                summary: result.summary.clone(),
                model_content: result.model_content.clone(),
                truncated: result.truncated,
                structured: result.structured.clone(),
            })?;
            Ok(())
        })();
        if let Err(error) = write_result {
            self.state = RunState::WaitingForPermission(Box::new(waiting));
            return Err(error);
        }

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
            pending.record_tool_call(&tool_call.name);
            self.state = RunState::Pending(Box::new(pending));
            return Ok(Some(UiEvent::ToolEnd {
                id: tool_call.id,
                status: "blocked".to_string(),
                summary: block.reason,
                model_content: None,
                result: None,
            }));
        }
        let summary = display_summary(&tool_call.name, &hook_result.args, None);
        pending.record_tool_call(&tool_call.name);
        let (slot, tool_kind) = super::slots::dispatch_tool_slot(super::slots::SlotDispatchArgs {
            tool_name: tool_call.name.clone(),
            tool_id: tool_call.id.clone(),
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
    let _ = append_turn_end(&streaming.pending.events_path, streaming.pending.turn);
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
    use crate::provider::types::ProviderToolCall;
    use crate::query::types::CumulativeUsage;

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
            subagent_registry: None,
            skill_registry: None,
            skill_content_hash: None,
            skill_body: None,
            child_session_count: 0,
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

    #[tokio::test]
    async fn cancel_when_idle_produces_turn_end() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        {
            let mut store = EventStore::open(&events_path).unwrap();
            store
                .append(EventPayload::SessionMeta {
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    schema_version: 1,
                    session_id: "test".to_string(),
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                    kuku_version: "0.1.0".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::TurnStart {
                    turn: 1,
                    ts: "2026-05-20T00:00:00Z".to_string(),
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
            EventPayload::TurnEnd { turn: 1, .. }
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
    async fn resume_after_cancel_includes_turn_end_in_history() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        {
            let mut store = EventStore::open(&events_path).unwrap();
            store
                .append(EventPayload::SessionMeta {
                    ts: "2026-05-20T00:00:00Z".to_string(),
                    schema_version: 1,
                    session_id: "test".to_string(),
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                    kuku_version: "0.1.0".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::TurnStart {
                    turn: 1,
                    ts: "2026-05-20T00:00:00Z".to_string(),
                })
                .unwrap();
            store
                .append(EventPayload::UserInput {
                    turn: 1,
                    ts: "2026-05-20T00:00:01Z".to_string(),
                    text: "hello".to_string(),
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
                .append(EventPayload::TurnEnd {
                    turn: 1,
                    ts: "2026-05-20T00:00:03Z".to_string(),
                })
                .unwrap();
        }

        let events = EventStore::replay(&events_path).unwrap();
        let (summary, history) = crate::context::rebuild_history(&events);
        assert!(summary.is_none());
        assert_eq!(history.len(), 2);
        let messages: Vec<_> = history.iter().map(|m| format!("{:?}", m.role)).collect();
        assert!(messages.contains(&"User".to_string()));
        assert!(messages.contains(&"Assistant".to_string()));
    }

    #[test]
    fn child_session_id_is_predictable() {
        let parent_id = "abc123";
        let counter = 0u64;
        let expected = format!("child_{}_{}", parent_id, counter);
        assert_eq!(expected, "child_abc123_0");
    }
}
