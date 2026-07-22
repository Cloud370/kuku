use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::Arc;

use kuku::event::{ExecutionScope, RunId, TaskEvent, TaskId, WorkspaceId};
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit};

use super::driver::{DriverCommand, DriverEvent};
use super::DomainError;

pub(super) const PHASE_QUEUED: u8 = 0;
pub(super) const PHASE_LAUNCHING: u8 = 1;
pub(super) const PHASE_CANCELLED: u8 = 2;

const MAX_COALESCED_ACTIVITY_EVENTS: usize = 1_000;

pub(super) struct ActiveDriver {
    pub(super) task_id: TaskId,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) phase: Arc<AtomicU8>,
    pub(super) cancel: Option<watch::Sender<bool>>,
    pub(super) commands: Option<mpsc::Sender<DriverCommand>>,
    pub(super) _admission: OwnedSemaphorePermit,
}

pub(super) fn execution_scope_for_run(
    events: &[kuku::event::StoredEvent],
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    run_id: &RunId,
) -> Result<ExecutionScope, DomainError> {
    kuku::event::task_execution_scope(events, workspace_id, task_id, run_id, "main")
        .map_err(|_| DomainError::StorageExhausted)
}

pub(super) fn coalesce_activity_events(
    mut events: Vec<TaskEvent>,
    receiver: &mut mpsc::Receiver<DriverEvent>,
    pending_event: &mut Option<DriverEvent>,
) -> Vec<TaskEvent> {
    while events.len() < MAX_COALESCED_ACTIVITY_EVENTS {
        match receiver.try_recv() {
            Ok(DriverEvent::Activity(mut next))
                if events.len().saturating_add(next.len()) <= MAX_COALESCED_ACTIVITY_EVENTS =>
            {
                events.append(&mut next);
            }
            Ok(next) => {
                *pending_event = Some(next);
                break;
            }
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                break;
            }
        }
    }
    events
}
