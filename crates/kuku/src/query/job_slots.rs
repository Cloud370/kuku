use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::{sleep_until, Instant};

use crate::session::runtime::JobRequest;
use crate::tool::{ToolFaults, ToolResultEnvelope};
use crate::Result;

use super::runtime_tools;
use super::types::{PendingRun, QueuedToolCall, Run, RunState, SlotEvent, UiEvent};

const BACKGROUND_OBSERVATION: Duration = Duration::from_secs(1);

pub(super) struct ForegroundJob {
    tool: String,
    output_id: Option<String>,
    background: bool,
    started: Instant,
    deadline: Instant,
    completion: Option<oneshot::Receiver<SlotEvent>>,
    detached: Arc<AtomicBool>,
}

impl ForegroundJob {
    pub(super) fn start(
        pending: &PendingRun,
        queued: &QueuedToolCall,
        output_id: Option<String>,
    ) -> (Box<Self>, oneshot::Sender<SlotEvent>) {
        let (sender, receiver) = oneshot::channel();
        let background = queued.tool_call.args["background"] == true;
        let observation = if background {
            BACKGROUND_OBSERVATION
        } else {
            Duration::from_millis(pending.query.runtime_limits.job_foreground_wait_ms)
        };
        let started = Instant::now();
        let job = Self {
            tool: queued.tool_call.name.clone(),
            output_id,
            background,
            started,
            deadline: started + observation,
            completion: Some(receiver),
            detached: Arc::new(AtomicBool::new(false)),
        };
        (Box::new(job), sender)
    }

    pub(super) fn detached_flag(&self) -> Arc<AtomicBool> {
        self.detached.clone()
    }
}

enum SlotProgress {
    Event(String, SlotEvent),
    Promote(String),
    Input,
    Cancel,
}

impl Run {
    pub(super) fn holds_job(&self) -> bool {
        matches!(&self.state, RunState::Pending(pending) if pending.waiting_for_job_input)
    }

    pub(super) async fn poll_tool(
        &mut self,
        wait_for_input: bool,
    ) -> Result<Option<(UiEvent, ToolFaults)>> {
        match self.wait_for_slot(wait_for_input).await? {
            SlotProgress::Event(id, event) if self.slots.contains_key(&id) => {
                self.on_slot_event(&id, event)
            }
            SlotProgress::Event(..) => Ok(None),
            SlotProgress::Promote(id) => self
                .promote_job(&id)
                .map(|event| Some((event, ToolFaults::default()))),
            SlotProgress::Input => {
                if let RunState::Pending(pending) = &mut self.state {
                    pending.waiting_for_job_input = false;
                }
                Ok(None)
            }
            SlotProgress::Cancel => {
                self.cancel();
                Ok(None)
            }
        }
    }

    fn on_slot_event(
        &mut self,
        id: &str,
        event: SlotEvent,
    ) -> Result<Option<(UiEvent, ToolFaults)>> {
        let job_control = matches!(&event, SlotEvent::JobControlDone(_));
        let event = match event {
            SlotEvent::Output(event) => UiEvent::ToolOutput {
                id: id.into(),
                event,
            },
            SlotEvent::WorkspacePermission(action) => {
                let action = *action;
                if let RunState::Pending(pending) = &mut self.state {
                    self.slots.remove(id);
                    action.persist(&pending.events_path)?;
                    super::helpers::append_permission_request(
                        &pending.events_path,
                        &pending.conversation,
                        pending.turn,
                        &action.request,
                    )?;
                    pending
                        .resumed_permission_requests
                        .push_front(action.request);
                    pending.queued_tool_calls.push_front(action.queued);
                    return Ok(None);
                }
                self.finish_slot(id, action.partial, false)?
            }
            SlotEvent::Done(result) | SlotEvent::JobControlDone(result) => {
                let faults = result.faults;
                let event = self.finish_slot(id, result, job_control)?;
                if faults.output_persistence_failed || faults.cleanup_incomplete {
                    self.cancel();
                }
                return Ok(Some((event, faults)));
            }
        };
        Ok(Some((event, ToolFaults::default())))
    }

    pub(super) fn job_wait_required(&self) -> Result<bool> {
        let RunState::Pending(pending) = &self.state else {
            return Ok(false);
        };
        if !pending.waiting_for_job_input {
            return Ok(false);
        }
        if self.slots.is_empty() && pending.queued_tool_calls.is_empty() {
            return runtime_tools::should_yield(pending).map(|yield_turn| !yield_turn);
        }
        Ok(true)
    }

    async fn wait_for_slot(&mut self, wait_for_input: bool) -> Result<SlotProgress> {
        let has_active_slot = !self.slots.is_empty();
        let foreground = self.slots.values_mut().find_map(|slot| {
            slot.job
                .as_mut()
                .map(|job| (slot.tool_call_id.clone(), job))
        });
        let input_boundary = match &self.state {
            RunState::Pending(pending) if wait_for_input => Some((
                runtime_tools::runtime_session(pending)?,
                pending.conversation.as_str(),
                pending
                    .query
                    .runtime_execution
                    .expect("Session query has execution identity"),
                pending.turn,
            )),
            _ => None,
        };
        let deadline = foreground
            .as_ref()
            .filter(|_| matches!(self.state, RunState::Pending(_)))
            .map(|(_, job)| job.deadline);
        let mut receiver = foreground.map(|(id, job)| {
            (
                id,
                job.completion.as_mut().expect("foreground owns completion"),
            )
        });
        let progress = tokio::select! {
            biased;
            _ = self.cancel_token.notified() => SlotProgress::Cancel,
            input = async {
                let Some((session, agent, execution, turn)) = input_boundary else { return pending().await };
                session.wait_for_job_input(agent, execution, turn).await
            } => {
                input?;
                SlotProgress::Input
            }
            _ = async {
                if let Some(deadline) = deadline {
                    sleep_until(deadline).await;
                } else {
                    pending::<()>().await;
                }
            } => ready_or_promote(receiver),
            event = self.slot_event_rx.recv(), if has_active_slot => match event {
                Some((id, event)) => SlotProgress::Event(id, event),
                None => SlotProgress::Cancel,
            },
            outcome = async {
                let Some((id, completion)) = &mut receiver else { return pending().await };
                SlotProgress::Event(id.clone(), completion.await.unwrap_or_else(|_| worker_exited()))
            } => outcome,
        };
        Ok(progress)
    }

    fn promote_job(&mut self, id: &str) -> Result<UiEvent> {
        let RunState::Pending(pending) = &mut self.state else {
            unreachable!("Job promotion occurs at a pending tool boundary");
        };
        let slot = self.slots.get_mut(id).expect("running tool slot exists");
        let job = slot.job.as_mut().expect("slot supports Job promotion");
        let session = runtime_tools::runtime_session(pending)?;
        let handle = session.detach_job(
            JobRequest {
                agent: pending.conversation.as_str().to_owned(),
                execution_id: pending
                    .query
                    .runtime_execution
                    .expect("Session query has execution identity"),
                turn: pending.turn,
                tool_call_id: id.to_owned(),
                tool: job.tool.clone(),
                output_id: job.output_id.clone(),
                summary: slot.label.clone(),
                started: job.started,
                wait_tool_available: super::helpers::resolved_tool_available(pending, "wait_job"),
            },
            slot.cancel.clone(),
            || {
                job.detached.store(true, Ordering::Release);
                let receiver = job.completion.take().expect("foreground owns completion");
                async move {
                    match receiver.await.unwrap_or_else(|_| worker_exited()) {
                        SlotEvent::Done(result) => result,
                        SlotEvent::WorkspacePermission(action) => action.blocked_job(),
                        SlotEvent::Output(_) | SlotEvent::JobControlDone(_) => {
                            unreachable!("unexpected Job completion event")
                        }
                    }
                }
            },
        )?;
        pending.waiting_for_job_input |= !job.background;
        self.slots.remove(id);
        Ok(UiEvent::ToolEnd {
            id: id.into(),
            status: handle.status,
            summary: handle.summary,
            model_content: Some(handle.model_content),
            result: handle.structured,
        })
    }
}

fn ready_or_promote(receiver: Option<(String, &mut oneshot::Receiver<SlotEvent>)>) -> SlotProgress {
    let Some((id, receiver)) = receiver else {
        return SlotProgress::Input;
    };
    match receiver.try_recv() {
        Ok(event) => SlotProgress::Event(id, event),
        Err(oneshot::error::TryRecvError::Empty) => SlotProgress::Promote(id),
        Err(oneshot::error::TryRecvError::Closed) => SlotProgress::Event(id, worker_exited()),
    }
}

fn worker_exited() -> SlotEvent {
    SlotEvent::Done(ToolResultEnvelope::output_failure(
        "tool worker stopped before reporting its outcome",
        "execution and output could not be confirmed",
        "tool_worker_lost",
        true,
    ))
}

#[cfg(test)]
mod tests {
    use super::{ready_or_promote, SlotEvent, SlotProgress, ToolResultEnvelope};

    #[test]
    fn ready_foreground_result_wins_at_the_promotion_boundary() {
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        assert!(matches!(
            ready_or_promote(Some(("tool".into(), &mut receiver))),
            SlotProgress::Promote(id) if id == "tool"
        ));
        let result = ToolResultEnvelope::error("command failed", "exit 1");
        sender.send(SlotEvent::Done(result.clone())).unwrap();
        assert!(matches!(
            ready_or_promote(Some(("tool".into(), &mut receiver))),
            SlotProgress::Event(id, SlotEvent::Done(finished)) if id == "tool" && finished == result
        ));
    }
}
