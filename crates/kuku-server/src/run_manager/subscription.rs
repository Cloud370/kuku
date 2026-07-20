use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{broadcast, OwnedSemaphorePermit, Semaphore};

use kuku::event::{Cursor, TaskId};

use crate::api::{ApiVersion, TaskDelta, TaskProjection, TaskStreamEvent};

use super::repository::{TaskPublication, TaskRepository};
use super::DomainError;

const PUBLICATION_BUFFER: usize = 256;

type TaskSender = broadcast::Sender<TaskPublication>;

pub struct TaskSubscriptionHub {
    repository: TaskRepository,
    channels: Mutex<HashMap<TaskId, TaskSender>>,
    task_limits: Mutex<HashMap<TaskId, Arc<Semaphore>>>,
    total_limit: Arc<Semaphore>,
    task_stream_limit: usize,
}

impl TaskSubscriptionHub {
    pub fn attach(
        repository: TaskRepository,
        total_stream_limit: usize,
        task_stream_limit: usize,
    ) -> Arc<Self> {
        let hub = Arc::new(Self {
            repository: repository.clone(),
            channels: Mutex::new(HashMap::new()),
            task_limits: Mutex::new(HashMap::new()),
            total_limit: Arc::new(Semaphore::new(total_stream_limit)),
            task_stream_limit,
        });
        let weak = Arc::downgrade(&hub);
        repository.register_observer(Arc::new(move |publication| {
            if let Some(hub) = Weak::upgrade(&weak) {
                hub.publish(publication);
            }
        }));
        hub
    }

    pub async fn subscribe(
        self: &Arc<Self>,
        task_id: &TaskId,
        after: Option<Cursor>,
    ) -> Result<TaskSubscription, DomainError> {
        let total_permit = self
            .total_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| DomainError::StreamLimit)?;
        let task_limit = self.task_limit(task_id);
        let task_permit = match task_limit.try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Err(DomainError::StreamLimit),
        };

        let sender = self.sender(task_id);
        let (receiver, replacement) = match self.initial_state(task_id, sender.clone()) {
            Ok(state) => state,
            Err(error) => return Err(error),
        };
        if after.is_some_and(|cursor| cursor.get() > replacement.cursor.get()) {
            return Err(DomainError::CursorAhead);
        }
        let cutoff = replacement.cursor;
        Ok(TaskSubscription {
            task_id: task_id.clone(),
            repository: self.repository.clone(),
            sender,
            receiver,
            pending: Some(replacement),
            cutoff,
            _total_permit: total_permit,
            _task_permit: task_permit,
        })
    }

    fn task_limit(&self, task_id: &TaskId) -> Arc<Semaphore> {
        let mut limits = self
            .task_limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        limits
            .entry(task_id.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(self.task_stream_limit)))
            .clone()
    }

    fn sender(&self, task_id: &TaskId) -> TaskSender {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        channels
            .entry(task_id.clone())
            .or_insert_with(|| broadcast::channel(PUBLICATION_BUFFER).0)
            .clone()
    }

    fn publish(&self, publication: &TaskPublication) {
        let sender = self
            .channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&publication.projection.task.task_id)
            .cloned();
        if let Some(sender) = sender {
            let _ = sender.send(publication.clone());
        }
    }

    fn initial_state(
        &self,
        task_id: &TaskId,
        sender: TaskSender,
    ) -> Result<(broadcast::Receiver<TaskPublication>, TaskStreamEvent), DomainError> {
        if !self.repository.task_path(task_id).is_dir() {
            return Err(DomainError::TaskNotFound);
        }
        let store = self.repository.event_store(task_id)?;
        store.with_publication_transaction(|| {
            let receiver = sender.subscribe();
            let projection = self.repository.rebuild(task_id)?.projection()?;
            Ok((receiver, replacement_event(projection)))
        })
    }
}

#[derive(Debug)]
pub struct TaskSubscription {
    task_id: TaskId,
    repository: TaskRepository,
    sender: TaskSender,
    receiver: broadcast::Receiver<TaskPublication>,
    pending: Option<TaskStreamEvent>,
    cutoff: Cursor,
    _total_permit: OwnedSemaphorePermit,
    _task_permit: OwnedSemaphorePermit,
}

impl TaskSubscription {
    pub async fn next(&mut self) -> Result<TaskStreamEvent, DomainError> {
        if let Some(event) = self.pending.take() {
            return Ok(event);
        }
        loop {
            match self.receiver.recv().await {
                Ok(publication) => {
                    let cursor = publication.projection.cursor;
                    if cursor.get() <= self.cutoff.get() {
                        continue;
                    }
                    self.cutoff = cursor;
                    return Ok(publication_event(publication));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let (receiver, replacement) = self.repository_replacement()?;
                    self.receiver = receiver;
                    self.cutoff = replacement.cursor;
                    return Ok(replacement);
                }
                Err(broadcast::error::RecvError::Closed) => return Err(DomainError::LedgerCorrupt),
            }
        }
    }

    fn repository_replacement(
        &self,
    ) -> Result<(broadcast::Receiver<TaskPublication>, TaskStreamEvent), DomainError> {
        let store = self.repository.event_store(&self.task_id)?;
        store.with_publication_transaction(|| {
            let receiver = self.sender.subscribe();
            let projection = self.repository.rebuild(&self.task_id)?.projection()?;
            Ok((receiver, replacement_event(projection)))
        })
    }
}

fn replacement_event(projection: TaskProjection) -> TaskStreamEvent {
    TaskStreamEvent {
        api_version: ApiVersion,
        cursor: projection.cursor,
        task_revision: projection.task_revision,
        task_id: projection.task.task_id.clone(),
        event: TaskDelta::ProjectionReplaced {
            projection: Box::new(projection),
        },
    }
}

fn publication_event(publication: TaskPublication) -> TaskStreamEvent {
    TaskStreamEvent {
        api_version: ApiVersion,
        cursor: publication.projection.cursor,
        task_revision: publication.projection.task_revision,
        task_id: publication.projection.task.task_id.clone(),
        event: publication.delta,
    }
}
