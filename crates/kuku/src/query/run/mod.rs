use std::sync::Arc;
mod advance;
mod helpers;
mod permission;
mod stream;
#[cfg(test)]
mod tests;
use crate::error::{Error, Result};
use crate::event::EventPayload;
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
        for slot in self.slots.values() {
            slot.cancel.notify_one();
            if let Some(cancellation) = &slot.command_cancellation {
                cancellation.cancel();
            }
        }
        self.cancel_token.notify_waiters();
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

    /// The Task identity for this execution.
    pub fn task_id(&self) -> &crate::event::TaskId {
        &self.execution_scope.task_id
    }

    /// The Run identity for this execution.
    pub fn run_id(&self) -> &crate::event::RunId {
        &self.execution_scope.run_id
    }

    /// The Turn identity for this execution.
    pub fn turn_id(&self) -> &crate::event::TurnId {
        &self.execution_scope.turn_id
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
            if let Some(cancellation) = &slot.command_cancellation {
                cancellation.cancel();
            }
        }
        let (event_store, turn) = match std::mem::replace(&mut self.state, RunState::Done(None)) {
            RunState::Pending(mut pending) => {
                self.persist_deferred_runtime_logs_for_pending(&mut pending);
                pending.flush_runtime_logs();
                (pending.event_store.clone(), pending.turn)
            }
            RunState::Streaming(mut streaming) => {
                self.persist_deferred_runtime_logs_for_pending(&mut streaming.pending);
                streaming.pending.flush_runtime_logs();
                (
                    streaming.pending.event_store.clone(),
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
                (waiting.pending.event_store.clone(), waiting.pending.turn)
            }
            other @ (RunState::Cancelled { .. } | RunState::Done(_)) => {
                self.state = other;
                return;
            }
        };
        self.state = RunState::Cancelled { event_store, turn };
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
                            let (event_store, turn) = match &self.state {
                                RunState::Pending(p) => (&p.event_store, p.turn),
                                RunState::Streaming(s) => (&s.pending.event_store, s.pending.turn),
                                RunState::WaitingForPermission(w) => {
                                    (&w.pending.event_store, w.pending.turn)
                                }
                                RunState::Cancelled {
                                    event_store, turn, ..
                                } => (event_store, *turn),
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
                                &self.execution_scope,
                                &slot,
                                &status,
                                &summary,
                                &model_content,
                                &result,
                                event_store,
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
                RunState::Cancelled {
                    event_store, turn, ..
                } => {
                    append_turn_cancelled(
                        &event_store,
                        &self.execution_scope,
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
}

pub(crate) fn find_tool_definition<'a>(
    pending: &'a PendingRun,
    name: &str,
) -> Option<&'a crate::tool::ToolDefinition> {
    helpers::find_tool_definition(pending, name)
}
