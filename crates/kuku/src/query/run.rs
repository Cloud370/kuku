use std::future::Future;
use std::sync::Arc;
#[cfg(test)]
mod completion_tests;
mod helpers;
mod provider_error;
mod queued_calls;
#[cfg(test)]
mod tests;
use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::permission::append_project_allow_rule;
use crate::provider::chunk::ProviderChunk;
use crate::provider::types::ProviderToolCall;
use crate::tool::{ToolFaults, ToolResultEnvelope};
use helpers::persist_blocked_tool_result;

use super::helpers::{
    append_permission_decision, append_turn_cancelled, display_summary, now_timestamp,
    permission_candidate,
};
use super::slots::requires_workspace_ordering;
use super::tool_exec::{execute_tool_call, run_tool_pre_hooks};
use super::types::{
    ModelPhase, PendingPermission, PendingRun, PendingStep, PermissionChoice, QueuedToolCall, Run,
    RunState, StreamingChunkState, UiEvent,
};

impl Drop for Run {
    fn drop(&mut self) {
        if self
            .lock_path
            .extension()
            .is_none_or(|extension| extension != "nested")
        {
            crate::session::release_lock(&self.lock_path);
        }
    }
}

impl Run {
    fn has_active_workspace_ordered_slot(&self) -> bool {
        self.holds_job() || self.slots.values().any(|slot| slot.workspace_ordered)
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

    /// Cancel the current run, abort streaming, and close pending permissions.
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
    #[allow(
        clippy::should_implement_trait,
        reason = "The existing async next API returns its polling future directly to avoid an extra frame on the caller's stack"
    )]
    pub fn next(&mut self) -> impl Future<Output = Result<Option<UiEvent>>> + '_ {
        self.next_event(|event, _| event)
    }

    pub(crate) fn next_with_tool_faults(
        &mut self,
    ) -> impl Future<Output = Result<Option<(UiEvent, ToolFaults)>>> + '_ {
        self.next_event(|event, faults| (event, faults))
    }

    async fn next_event<T>(&mut self, project: fn(UiEvent, ToolFaults) -> T) -> Result<Option<T>> {
        let event = loop {
            self.persist_deferred_runtime_logs();

            if matches!(&self.state, RunState::Pending(_)) {
                if let Some(event) = self.try_process_queued_call().await? {
                    break self.defer_runtime_log_if_needed(event);
                }
            }
            let wait_for_job_input = self.job_wait_required()?;
            if wait_for_job_input || !self.slots.is_empty() {
                if let Some((event, faults)) = self.poll_tool(wait_for_job_input).await? {
                    return Ok(Some(project(event, faults)));
                }
                continue;
            }
            match std::mem::replace(&mut self.state, RunState::Done(None)) {
                RunState::Pending(pending) => {
                    if let Some(event) = self.advance_from_pending(pending).await? {
                        break self.defer_runtime_log_if_needed(event);
                    }
                }
                RunState::Streaming(streaming) => {
                    if let Some(event) = self.advance_from_streaming(streaming).await? {
                        break self.defer_runtime_log_if_needed(event);
                    }
                }
                RunState::WaitingForPermission(waiting) => {
                    let request = waiting.request.clone();
                    self.state = RunState::WaitingForPermission(waiting);
                    break UiEvent::PermissionRequested { request };
                }
                RunState::Cancelled { events_path, turn } => {
                    append_turn_cancelled(&events_path, &self.agent, turn, "user_cancelled")?;
                    self.state = RunState::Done(None);
                    break UiEvent::Cancelled { turn };
                }
                RunState::Done(Some((output, usage, turn))) => {
                    self.state = RunState::Done(None);
                    break UiEvent::Done {
                        output,
                        usage,
                        turn,
                    };
                }
                RunState::Done(None) => return Ok(None),
            }
        };
        Ok(Some(project(event, ToolFaults::default())))
    }

    pub(super) fn finish_slot(
        &mut self,
        id: &str,
        envelope: ToolResultEnvelope,
        job_wait: bool,
    ) -> Result<UiEvent> {
        let slot = self.slots.remove(id).expect("slot must exist");
        self.state.record_tool_completion(&envelope.status);
        let job_session = match &self.state {
            RunState::Pending(pending) if job_wait => {
                Some(super::runtime_tools::runtime_session(pending)?)
            }
            _ => None,
        };
        let context = match &self.state {
            RunState::Pending(p) => Some((&p.events_path, p.turn)),
            RunState::Streaming(s) => Some((&s.pending.events_path, s.pending.turn)),
            RunState::WaitingForPermission(w) => Some((&w.pending.events_path, w.pending.turn)),
            RunState::Cancelled { events_path, turn } => Some((events_path, *turn)),
            RunState::Done(_) => self
                .cleanup_context
                .as_ref()
                .map(|(path, turn)| (path, *turn)),
        };
        let (result, model_content) = match context {
            Some((path, turn)) => {
                let result = super::tool_exec::write_tool_result(
                    &slot,
                    &envelope,
                    path,
                    turn,
                    job_session.as_ref(),
                )?;
                (
                    result,
                    (!envelope.model_content.is_empty()).then_some(envelope.model_content),
                )
            }
            None => (envelope.structured, None),
        };
        Ok(UiEvent::ToolEnd {
            id: slot.tool_call_id,
            status: envelope.status,
            summary: envelope.summary,
            model_content,
            result,
        })
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
        let step =
            super::step::advance_pending(*pending, self.slot_event_tx.clone(), self.slots.len())
                .await?;
        self.apply_pending_step(step).await
    }

    async fn apply_pending_step(&mut self, step: PendingStep) -> Result<Option<UiEvent>> {
        match step {
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
                let persisted =
                    provider_error::record_streaming_provider_error_facts(&streaming, &error);
                streaming.pending.flush_runtime_logs();
                persisted?;
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
                    PendingStep::Failed(error) => {
                        self.state = RunState::Done(None);
                        Err(error)
                    }
                    PendingStep::NeedPermission(_) => unreachable!(),
                    PendingStep::Streaming(streaming) => {
                        self.state = RunState::Streaming(streaming);
                        Ok(None)
                    }
                }
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
                        streaming.stream_failure = Some(failure.clone());
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
                    streaming.stop_reason = Some(crate::event::ModelStopReason::Unknown(
                        "cancelled".to_string(),
                    ));
                    return Ok(None);
                }
            };

            if streaming.stream_ended {
                streaming.terminal_stream_invalid = true;
                continue;
            }

            match chunk {
                ProviderChunk::StreamStart { request_id: rid } => {
                    if streaming.stop_reason.is_some() || streaming.provider_request_id.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    streaming.provider_request_id = Some(rid);
                }
                ProviderChunk::TextDelta { text } => {
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    if let Some(ref mut detector) = streaming.handoff_detector {
                        if let Some(user_text) = detector.process(&text) {
                            if !user_text.is_empty() {
                                streaming.model_phase = Some(ModelPhase::Streaming);
                                streaming.accumulated_text.push_str(&user_text);
                                return Ok(Some(UiEvent::TextDelta { text: user_text }));
                            }
                        }
                        continue;
                    }
                    if !text.is_empty() {
                        streaming.model_phase = Some(ModelPhase::Streaming);
                    }
                    streaming.accumulated_text.push_str(&text);
                    return Ok(Some(UiEvent::TextDelta { text }));
                }
                ProviderChunk::Reasoning => {
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    streaming
                        .thinking_start
                        .get_or_insert_with(std::time::Instant::now);
                    if let Some(event) = streaming.model_progress(ModelPhase::Thinking) {
                        return Ok(Some(event));
                    }
                }
                ProviderChunk::ThinkingDelta { text } => {
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    if !text.is_empty() {
                        streaming
                            .thinking_start
                            .get_or_insert_with(std::time::Instant::now);
                        streaming.model_phase = Some(ModelPhase::Thinking);
                    }
                    streaming.accumulated_thinking.push_str(&text);
                    return Ok(Some(UiEvent::ThinkingDelta { text }));
                }
                ProviderChunk::ToolCallStart { index, id, name } => {
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    if streaming
                        .tool_calls
                        .iter()
                        .any(|tool_call| tool_call.index == index)
                    {
                        streaming.tool_stream_invalid = true;
                    }
                    streaming.tool_calls.push(ProviderToolCall {
                        id,
                        name,
                        args: serde_json::json!({}),
                        index,
                    });
                    streaming.tool_arg_buffers.push((index, String::new()));
                    if !streaming.tool_stream_invalid {
                        if let Some(event) = streaming.model_progress(ModelPhase::ToolArguments) {
                            return Ok(Some(event));
                        }
                    }
                }
                ProviderChunk::ToolCallArgDelta { index, fragment } => {
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    let has_single_start = streaming
                        .tool_calls
                        .iter()
                        .filter(|tool_call| tool_call.index == index)
                        .count()
                        == 1;
                    if !has_single_start || streaming.tool_call_completions.contains(&index) {
                        streaming.tool_stream_invalid = true;
                    }
                    let has_fragment = !fragment.is_empty();
                    if let Some((_, buf)) = streaming
                        .tool_arg_buffers
                        .iter_mut()
                        .find(|(i, _)| *i == index)
                    {
                        buf.push_str(&fragment);
                    } else {
                        streaming.tool_arg_buffers.push((index, fragment));
                    }
                    if has_fragment && !streaming.tool_stream_invalid {
                        if let Some(start) = streaming.thinking_start.take() {
                            streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                        }
                        if let Some(event) = streaming.model_progress(ModelPhase::ToolArguments) {
                            return Ok(Some(event));
                        }
                    }
                }
                ProviderChunk::ToolCallStop { index } => {
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    let has_single_start = streaming
                        .tool_calls
                        .iter()
                        .filter(|tool_call| tool_call.index == index)
                        .count()
                        == 1;
                    if has_single_start && !streaming.tool_call_completions.contains(&index) {
                        streaming.tool_call_completions.push(index);
                    } else {
                        streaming.tool_stream_invalid = true;
                    }
                }
                ProviderChunk::ContentBlockStop { index } => {
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                    let _ = index;
                }
                ProviderChunk::InvalidToolStream => {
                    streaming.tool_stream_invalid = true;
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    }
                }
                ProviderChunk::StopReason { reason } => {
                    if let Some(start) = streaming.thinking_start.take() {
                        streaming.thinking_duration_ms += start.elapsed().as_millis() as u64;
                    }
                    if streaming.stop_reason.is_some() {
                        streaming.terminal_stream_invalid = true;
                    } else {
                        streaming.stop_reason = Some(reason);
                    }
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
                                input_tokens: None,
                                output_tokens: None,
                                cache_read_input_tokens: None,
                                cache_creation_input_tokens: None,
                            });
                    if input_tokens.is_some() {
                        entry.input_tokens = input_tokens;
                    }
                    let updated_output =
                        output_tokens.filter(|count| entry.output_tokens != Some(*count));
                    if output_tokens.is_some() {
                        entry.output_tokens = output_tokens;
                    }
                    if cache_read_input_tokens.is_some() {
                        entry.cache_read_input_tokens = cache_read_input_tokens;
                    }
                    if cache_creation_input_tokens.is_some() {
                        entry.cache_creation_input_tokens = cache_creation_input_tokens;
                    }
                    if let Some(output_tokens) = updated_output {
                        return Ok(Some(UiEvent::ModelOutputUsage { output_tokens }));
                    }
                }
                ProviderChunk::Replay { protocol, value } => {
                    let replay = EventPayload::Unknown(serde_json::json!({
                        "kind": "runtime.provider_replay", "ts": now_timestamp()?,
                        "agent": streaming.conversation.as_str(), "turn": streaming.pending.turn,
                        "request_id": streaming.request_id, "protocol": protocol, "value": value,
                    }));
                    if streaming.pending.query.runtime_execution.is_some() {
                        streaming.provider_replay.push(replay);
                    } else {
                        EventStore::open(&streaming.pending.events_path)?.append(replay)?;
                    }
                }
                ProviderChunk::Report { report } => {
                    streaming.accounting.report = Some(report.clone());
                    EventStore::open(&streaming.pending.events_path)?.append(EventPayload::Unknown(serde_json::json!({
                        "kind": "runtime.provider_report", "ts": now_timestamp()?,
                        "agent": streaming.conversation.as_str(), "turn": streaming.pending.turn,
                        "request_id": streaming.request_id, "report": report,
                    })))?;
                }
                ProviderChunk::ServerError { code, message } => {
                    let failure = crate::provider::classify_stream_error(&code, &message);
                    streaming.stream_failure = Some(failure.clone());
                    return Err(crate::error::Error::Provider {
                        kind: failure.kind,
                        message: failure.message,
                        provider: None,
                        model: None,
                    });
                }
                ProviderChunk::StreamEnd => {
                    streaming.stream_ended = true;
                    if let Some(timing) = &mut streaming.request_timing {
                        timing.elapsed_us =
                            u64::try_from(timing.started.elapsed().as_micros()).ok();
                    }
                }
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
        let result = super::workspace_script::cancelled_permission(waiting)?;
        let mut store = EventStore::open(&waiting.pending.events_path)?;
        store.append(EventPayload::ToolResult {
            turn: waiting.pending.turn,
            ts: now_timestamp()?,
            conversation: Some(waiting.pending.conversation.as_str().into()),
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
        if pending.query.runtime.is_some() {
            super::provider::ensure_resolved(&mut pending)?;
            super::runtime_tools::validate_prepared(&pending, &tool_call)?;
        }
        let (granted_tool, candidate) = if waiting.request.source == "run_js" {
            (
                waiting.request.tool.clone(),
                waiting.request.candidate.clone(),
            )
        } else {
            (
                tool_call.name.clone(),
                permission_candidate(
                    &pending.kuku_home,
                    &pending.workspace,
                    &tool_call.name,
                    &tool_call.args,
                ),
            )
        };
        let rule = format!("{granted_tool}({candidate})");
        if matches!(choice, PermissionChoice::Project) {
            append_project_allow_rule(&pending.policy_path, &granted_tool, &candidate)?;
        }
        append_permission_decision(
            &pending.events_path,
            pending.turn,
            &tool_call.id,
            choice,
            source,
            &rule,
        )?;
        if pending.query.runtime.is_some() {
            let queued = QueuedToolCall {
                tool_call,
                display_summary: queued_summary,
            };
            let step = if matches!(choice, PermissionChoice::Deny) {
                super::runtime_tools::reject(pending, queued)?
            } else {
                super::runtime_tools::dispatch(
                    pending,
                    queued,
                    self.slot_event_tx.clone(),
                    self.slots.len(),
                )
                .await?
            };
            return self.apply_pending_step(step).await;
        }
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
        if requires_workspace_ordering(&tool_call.name) && self.has_active_workspace_ordered_slot()
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
            capacity: crate::WorkspaceExecutionHost::default(),
            output: None,
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
