use std::collections::{hash_map::Entry, HashMap, HashSet, VecDeque};
#[cfg(test)]
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

use kuku::event::{
    EventPayload, ExecutionScope, InteractionId, MessageRoleFact, ProviderFailureFact,
    ProviderFailureKind, RequestFailed, RequestId, RequestScope, RunFact, RunId, RunState,
    TaskEvent, TaskId, TaskLedgerRecord,
};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

use crate::api::{
    CommandAccepted, CreateTaskResponse, ListTasksQuery, ReviewSubmissionResult, SubmitRunResponse,
    TaskPage, TaskProjection, TimelinePage, TimelineQuery,
};
use crate::platform::WorkspaceRegistry;

use super::driver::{DriverCommand, DriverEvent, DriverStart, RunDriverFactory, RunResult};
use super::store::{CreateTaskCommand, ResolveInteractionCommand, StopRunCommand};
use super::submission::{
    ReviewSubmissionValidator, RunQueueAdmission, RunQueueReservation, SkillSelectionValidator,
    SubmitReviewCommand, SubmitRunCommand,
};
use super::subscription::{TaskSubscription, TaskSubscriptionHub};
use super::{DomainError, TaskCommandService, TaskRepository};

struct ActiveDriver {
    task_id: TaskId,
    cancelled: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    commands: Option<mpsc::Sender<DriverCommand>>,
    _admission: OwnedSemaphorePermit,
}

const PHASE_QUEUED: u8 = 0;
const PHASE_LAUNCHING: u8 = 1;
const PHASE_CANCELLED: u8 = 2;

pub(super) fn execution_scope_for_run(
    events: &[kuku::event::StoredEvent],
    workspace_id: &kuku::event::WorkspaceId,
    task_id: &TaskId,
    run_id: &RunId,
) -> Result<ExecutionScope, DomainError> {
    kuku::event::task_execution_scope(events, workspace_id, task_id, run_id, "main")
        .map_err(|_| DomainError::StorageExhausted)
}

pub struct RunSupervisor {
    repository: TaskRepository,
    factory: Arc<dyn RunDriverFactory>,
    running: Arc<Semaphore>,
    admission: Arc<Semaphore>,
    drivers: Mutex<HashMap<RunId, ActiveDriver>>,
    queue: Mutex<VecDeque<RunId>>,
    self_ref: Weak<RunSupervisor>,
    #[cfg(test)]
    launch_pause: Mutex<Option<(Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>)>>,
}

impl RunSupervisor {
    pub fn new(
        repository: TaskRepository,
        factory: Arc<dyn RunDriverFactory>,
        max_concurrent: usize,
        max_queued: usize,
    ) -> Result<Arc<Self>, DomainError> {
        if max_concurrent == 0 {
            return Err(DomainError::InvalidRequest);
        }
        let total = max_concurrent
            .checked_add(max_queued)
            .ok_or(DomainError::StorageExhausted)?;
        Ok(Arc::new_cyclic(|self_ref| Self {
            repository,
            factory,
            running: Arc::new(Semaphore::new(max_concurrent)),
            admission: Arc::new(Semaphore::new(total)),
            drivers: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            self_ref: self_ref.clone(),
            #[cfg(test)]
            launch_pause: Mutex::new(None),
        }))
    }

    fn admit(self: &Arc<Self>, task_id: TaskId, run_id: RunId, admission: OwnedSemaphorePermit) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let phase = Arc::new(AtomicU8::new(PHASE_QUEUED));
        let entry = ActiveDriver {
            task_id: task_id.clone(),
            cancelled: cancelled.clone(),
            phase: phase.clone(),
            commands: None,
            _admission: admission,
        };
        let inserted = {
            let mut drivers = self
                .drivers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Entry::Vacant(slot) = drivers.entry(run_id.clone()) {
                slot.insert(entry);
                true
            } else {
                false
            }
        };
        if inserted {
            self.queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back(run_id);
            self.dispatch();
        }
    }

    fn dispatch(self: &Arc<Self>) {
        loop {
            let Ok(permit) = self.running.clone().try_acquire_owned() else {
                return;
            };
            let next = self
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front();
            let Some(run_id) = next else {
                return;
            };
            let launch = {
                let drivers = self
                    .drivers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                drivers.get(&run_id).map(|entry| {
                    (
                        entry.task_id.clone(),
                        entry.cancelled.clone(),
                        entry.phase.clone(),
                    )
                })
            };
            let Some((task_id, cancelled, phase)) = launch else {
                continue;
            };
            let supervisor = self.clone();
            tokio::spawn(async move {
                supervisor
                    .clone()
                    .drive(task_id, run_id.clone(), cancelled, phase, permit)
                    .await;
                supervisor.remove(&run_id);
                supervisor.dispatch();
            });
        }
    }

    async fn drive(
        self: Arc<Self>,
        task_id: TaskId,
        run_id: RunId,
        cancelled: Arc<AtomicBool>,
        phase: Arc<AtomicU8>,
        permit: OwnedSemaphorePermit,
    ) {
        if cancelled.load(Ordering::Acquire) {
            self.persist_stopped(&task_id, &run_id).await;
            return;
        }
        let start = match self.driver_start(&task_id, &run_id) {
            Ok(start) => start,
            Err(error) => {
                self.persist_failed(&task_id, &run_id, &error.to_string())
                    .await;
                return;
            }
        };
        if phase
            .compare_exchange(
                PHASE_QUEUED,
                PHASE_LAUNCHING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            self.persist_stopped(&task_id, &run_id).await;
            return;
        }
        #[cfg(test)]
        let launch_pause = {
            self.launch_pause
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        };
        #[cfg(test)]
        if let Some((arrived, release)) = launch_pause {
            arrived.wait().await;
            release.wait().await;
        }
        if !self.persist_started(&task_id, &run_id).await {
            self.persist_stopped(&task_id, &run_id).await;
            return;
        }
        let mut handle = match self.factory.start(start).await {
            Ok(handle) => handle,
            Err(error) => {
                if cancelled.load(Ordering::Acquire) {
                    self.persist_stopped(&task_id, &run_id).await;
                } else {
                    self.persist_failed(&task_id, &run_id, &error.to_string())
                        .await;
                }
                return;
            }
        };
        {
            let mut drivers = self
                .drivers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = drivers.get_mut(&run_id) else {
                return;
            };
            if entry.task_id != task_id {
                return;
            }
            entry.commands = Some(handle.commands.clone());
        }
        if cancelled.load(Ordering::Acquire) {
            let _ = handle.commands.send(DriverCommand::Stop).await;
        }
        let mut terminal = false;
        while let Some(event) = handle.events.recv().await {
            let result = match event {
                DriverEvent::Started => Ok(()),
                DriverEvent::Activity(events) => self.append_activity(&task_id, events).await,
                DriverEvent::InteractionOpened(interaction) => {
                    self.append_activity(
                        &task_id,
                        vec![TaskEvent::InteractionOpened { interaction }],
                    )
                    .await
                }
                DriverEvent::InteractionClosed(_) => Ok(()),
                DriverEvent::Completed(result) => {
                    terminal = true;
                    if cancelled.load(Ordering::Acquire) {
                        self.persist_stopped(&task_id, &run_id).await;
                    } else {
                        self.persist_completed(&task_id, &run_id, result).await;
                    }
                    Ok(())
                }
                DriverEvent::Stopped => {
                    terminal = true;
                    self.persist_stopped(&task_id, &run_id).await;
                    Ok(())
                }
                DriverEvent::Failed(failure) => {
                    terminal = true;
                    self.persist_failed(&task_id, &run_id, &failure.summary)
                        .await;
                    Ok(())
                }
            };
            if let Err(error) = result {
                terminal = true;
                let summary = format!("driver event persistence failed: {error}");
                self.persist_failed(&task_id, &run_id, &summary).await;
            }
            if terminal {
                break;
            }
        }
        if !terminal {
            self.persist_failed(
                &task_id,
                &run_id,
                "driver event stream ended before a terminal event",
            )
            .await;
        }
        drop(permit);
    }

    pub async fn stop(&self, run_id: &RunId) {
        let was_queued = {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(index) = queue.iter().position(|queued| queued == run_id) {
                queue.remove(index);
                true
            } else {
                false
            }
        };
        let (task_id, command, phase, cancelled) = {
            let drivers = self
                .drivers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = drivers.get(run_id) else {
                return;
            };
            (
                entry.task_id.clone(),
                entry.commands.clone(),
                entry.phase.clone(),
                entry.cancelled.clone(),
            )
        };
        let _ = phase.compare_exchange(
            PHASE_QUEUED,
            PHASE_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        cancelled.store(true, Ordering::Release);
        if was_queued {
            self.persist_stopped(&task_id, run_id).await;
            self.remove(run_id);
            return;
        }
        if let Some(command) = command {
            let _ = command.send(DriverCommand::Stop).await;
        }
    }

    pub async fn shutdown(&self) {
        let run_ids = self
            .drivers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for run_id in run_ids {
            self.stop(&run_id).await;
        }
        for _ in 0..100 {
            let empty = self
                .drivers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty();
            if empty {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub async fn resolve(&self, run_id: &RunId, interaction_id: InteractionId, choice_id: String) {
        let command = self
            .drivers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .and_then(|entry| entry.commands.clone());
        if let Some(command) = command {
            let _ = command
                .send(DriverCommand::Resolve {
                    interaction_id,
                    choice_id,
                })
                .await;
        }
    }

    fn remove(&self, run_id: &RunId) {
        self.drivers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id);
    }

    fn driver_start(&self, task_id: &TaskId, run_id: &RunId) -> Result<DriverStart, DomainError> {
        let projection = self.repository.rebuild(task_id)?.projection()?;
        let mut prompt = None;
        let mut current_skill_ids = Vec::new();
        for record in self.repository.replay(task_id)? {
            let EventPayload::TaskLedger(record) = record.payload else {
                continue;
            };
            let events = match &record {
                TaskLedgerRecord::Control(transaction) => transaction.events(),
                TaskLedgerRecord::Activity(batch) => batch.events(),
            };
            for event in events {
                match event {
                    TaskEvent::MessageAppended { message }
                        if message.run_id.as_ref() == Some(run_id)
                            && message.role == MessageRoleFact::User =>
                    {
                        prompt = Some(message.text.clone());
                    }
                    TaskEvent::SkillsChanged { selection } => {
                        current_skill_ids = selection.skill_ids.clone();
                    }
                    _ => {}
                }
            }
        }
        let event_store = self.repository.event_store(task_id)?;
        let execution_scope = execution_scope_for_run(
            &event_store
                .read_all()
                .map_err(|_| DomainError::LedgerCorrupt)?,
            &projection.task.workspace_id,
            task_id,
            run_id,
        )?;
        let selected_skills = super::driver::selected_skill_facts(
            &event_store,
            &execution_scope,
            &current_skill_ids,
        )?;
        Ok(DriverStart {
            task_id: task_id.clone(),
            run_id: run_id.clone(),
            workspace_id: projection.task.workspace_id,
            prompt: prompt.ok_or(DomainError::LedgerCorrupt)?,
            tier_id: projection.selected_tier_id,
            selected_skills,
            agent_message_id: format!("msg_agent_{}", run_id.as_str()),
            execution_scope,
            event_store,
        })
    }

    async fn append_activity(
        &self,
        task_id: &TaskId,
        events: Vec<TaskEvent>,
    ) -> Result<(), DomainError> {
        let _task = self.repository.task_guard(task_id).await;
        let batch = kuku::event::TaskActivityBatch::try_new(events)
            .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(task_id, TaskLedgerRecord::Activity(batch))?;
        Ok(())
    }

    async fn append_started(&self, task_id: &TaskId, run_id: &RunId) -> Result<bool, DomainError> {
        self.append_run_transition(task_id, run_id, RunState::Running, None, None)
            .await
    }

    async fn append_completed(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        result: RunResult,
    ) -> Result<(), DomainError> {
        self.append_run_transition(
            task_id,
            run_id,
            RunState::Completed,
            Some(result.summary.clone()),
            Some(result),
        )
        .await
        .map(|_| ())
    }

    async fn append_stopped(&self, task_id: &TaskId, run_id: &RunId) -> Result<(), DomainError> {
        self.append_run_transition(
            task_id,
            run_id,
            RunState::Stopped,
            Some("stopped".to_owned()),
            None,
        )
        .await
        .map(|_| ())
    }

    async fn append_failed(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        summary: &str,
    ) -> Result<(), DomainError> {
        self.append_run_transition(
            task_id,
            run_id,
            RunState::Failed,
            Some(summary.to_owned()),
            None,
        )
        .await
        .map(|_| ())
    }

    async fn persist_started(&self, task_id: &TaskId, run_id: &RunId) -> bool {
        loop {
            if let Ok(started) = self.append_started(task_id, run_id).await {
                return started;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn persist_completed(&self, task_id: &TaskId, run_id: &RunId, result: RunResult) {
        loop {
            if self
                .append_completed(task_id, run_id, result.clone())
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn persist_stopped(&self, task_id: &TaskId, run_id: &RunId) {
        loop {
            if self.append_stopped(task_id, run_id).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn persist_failed(&self, task_id: &TaskId, run_id: &RunId, summary: &str) {
        loop {
            if self.append_failed(task_id, run_id, summary).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn append_run_transition(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        intended: RunState,
        mut summary: Option<String>,
        result: Option<RunResult>,
    ) -> Result<bool, DomainError> {
        let _task = self.repository.task_guard(task_id).await;
        let aggregate = self.repository.rebuild(task_id)?;
        let agent_message_id = format!("msg_agent_{}", run_id.as_str());
        let finalize_agent = aggregate.message_needs_finalization(&agent_message_id);
        let projection = aggregate.projection()?;
        let Some(current) = projection.active_run else {
            return Ok(false);
        };
        if &current.run_id != run_id {
            return Ok(false);
        }
        let state = match (intended, current.state) {
            (RunState::Running, RunState::Queued) => RunState::Running,
            (RunState::Running, _) => return Ok(false),
            (RunState::Completed, RunState::Stopping) => {
                summary = Some("stopped".to_owned());
                RunState::Stopped
            }
            (RunState::Completed, _) => RunState::Completed,
            (RunState::Stopped, RunState::Stopping) => RunState::Stopped,
            (RunState::Stopped, _) => {
                summary = Some("driver stopped without a stop command".to_owned());
                RunState::Failed
            }
            (RunState::Failed, RunState::Stopping) => {
                summary = Some("stopped".to_owned());
                RunState::Stopped
            }
            (RunState::Failed, _) => RunState::Failed,
            _ => return Err(DomainError::LedgerCorrupt),
        };
        let mut run = RunFact {
            run_id: run_id.clone(),
            task_id: task_id.clone(),
            state,
            started_at: current.started_at,
            finished_at: (!state.is_active())
                .then(super::store::current_timestamp)
                .transpose()?,
            summary,
            warnings: Vec::new(),
            checks: None,
            metrics: None,
            workspace_changes: None,
        };
        if state == RunState::Completed {
            if let Some(result) = result {
                run.warnings = result.warnings;
                run.checks = result.checks;
                run.metrics = result.metrics;
                run.workspace_changes = result.workspace_changes;
            }
        }
        let event = match state {
            RunState::Running => TaskEvent::RunStarted { run },
            RunState::Completed => TaskEvent::RunCompleted { run },
            RunState::Stopped => TaskEvent::RunStopped { run },
            RunState::Failed => TaskEvent::RunFailed { run },
            _ => return Err(DomainError::LedgerCorrupt),
        };
        let mut events = Vec::with_capacity(2);
        if !state.is_active() && finalize_agent {
            events.push(TaskEvent::MessagePatched {
                message_id: agent_message_id,
                append_text: String::new(),
                finalized: true,
                request_ids: None,
            });
        }
        events.push(event);
        let batch = kuku::event::TaskActivityBatch::try_new(events)
            .map_err(|_| DomainError::LedgerCorrupt)?;
        self.repository
            .append(task_id, TaskLedgerRecord::Activity(batch))?;
        Ok(true)
    }
}

impl RunQueueAdmission for RunSupervisor {
    fn reserve(self: Arc<Self>) -> Result<Box<dyn RunQueueReservation>, DomainError> {
        let admission = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| DomainError::ServerBusy)?;
        Ok(Box::new(SupervisorReservation {
            supervisor: self,
            admission: Some(admission),
        }))
    }

    fn ensure_admitted(&self, task_id: &TaskId, run_id: &RunId) -> Result<(), DomainError> {
        if self
            .drivers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(run_id)
        {
            return Ok(());
        }
        let admission = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| DomainError::ServerBusy)?;
        let supervisor = self.self_ref.upgrade().ok_or(DomainError::RunNotActive)?;
        supervisor.admit(task_id.clone(), run_id.clone(), admission);
        Ok(())
    }
}

struct SupervisorReservation {
    supervisor: Arc<RunSupervisor>,
    admission: Option<OwnedSemaphorePermit>,
}

impl RunQueueReservation for SupervisorReservation {
    fn commit(mut self: Box<Self>, task_id: TaskId, run_id: RunId) {
        if let Some(admission) = self.admission.take() {
            self.supervisor.admit(task_id, run_id, admission);
        }
    }
}

#[derive(Clone)]
pub struct TaskRuntime {
    commands: TaskCommandService,
    supervisor: Arc<RunSupervisor>,
    subscriptions: Arc<TaskSubscriptionHub>,
}

impl TaskRuntime {
    pub fn new(
        repository: TaskRepository,
        factory: Arc<dyn RunDriverFactory>,
        workspaces: Arc<WorkspaceRegistry>,
        skills: Arc<dyn SkillSelectionValidator>,
        reviews: Arc<dyn ReviewSubmissionValidator>,
        max_concurrent: usize,
        max_queued: usize,
    ) -> Result<Self, DomainError> {
        let supervisor =
            RunSupervisor::new(repository.clone(), factory, max_concurrent, max_queued)?;
        let commands =
            TaskCommandService::new(repository, workspaces, skills, reviews, supervisor.clone());
        let subscriptions = TaskSubscriptionHub::attach(commands.repository().clone());
        Ok(Self {
            commands,
            supervisor,
            subscriptions,
        })
    }

    #[cfg(test)]
    pub(super) fn open_unchecked(
        home: impl AsRef<Path>,
        factory: Arc<dyn RunDriverFactory>,
        max_concurrent: usize,
        max_queued: usize,
    ) -> Result<Self, DomainError> {
        Self::from_repository_unchecked(
            TaskRepository::open(home)?,
            factory,
            max_concurrent,
            max_queued,
        )
    }

    #[cfg(test)]
    pub(super) fn from_repository_unchecked(
        repository: TaskRepository,
        factory: Arc<dyn RunDriverFactory>,
        max_concurrent: usize,
        max_queued: usize,
    ) -> Result<Self, DomainError> {
        let supervisor =
            RunSupervisor::new(repository.clone(), factory, max_concurrent, max_queued)?;
        let commands = TaskCommandService::new_unchecked_with_ports(
            repository,
            Arc::new(super::submission::TestSkillValidator),
            Arc::new(super::submission::TestReviewValidator),
            supervisor.clone(),
        );
        let subscriptions = TaskSubscriptionHub::attach(commands.repository().clone());
        Ok(Self {
            commands,
            supervisor,
            subscriptions,
        })
    }

    pub async fn create_task(
        &self,
        command: CreateTaskCommand,
    ) -> Result<CreateTaskResponse, DomainError> {
        self.commands.create_task(command).await
    }

    pub async fn submit(
        &self,
        command: SubmitRunCommand,
    ) -> Result<SubmitRunResponse, DomainError> {
        self.commands.submit(command).await
    }

    pub async fn submit_review(
        &self,
        command: SubmitReviewCommand,
    ) -> Result<ReviewSubmissionResult, DomainError> {
        self.commands.submit_review(command).await
    }

    pub async fn projection(&self, task_id: &TaskId) -> Result<TaskProjection, DomainError> {
        self.commands.projection(task_id).await
    }

    pub async fn subscribe(
        &self,
        task_id: &TaskId,
        after: Option<kuku::event::Cursor>,
    ) -> Result<TaskSubscription, DomainError> {
        self.subscriptions.subscribe(task_id, after).await
    }

    pub async fn list_tasks(
        &self,
        workspace_id: &kuku::event::WorkspaceId,
        search: Option<&str>,
    ) -> Result<TaskPage, DomainError> {
        self.commands.list_tasks(workspace_id, search).await
    }

    pub async fn list_tasks_query(&self, query: ListTasksQuery) -> Result<TaskPage, DomainError> {
        self.commands.list_tasks_query(query).await
    }

    pub async fn timeline(
        &self,
        task_id: &TaskId,
        query: TimelineQuery,
    ) -> Result<TimelinePage, DomainError> {
        self.commands.timeline(task_id, query).await
    }

    pub async fn stop(&self, command: StopRunCommand) -> Result<CommandAccepted, DomainError> {
        let (accepted, run_id) = self.commands.stop_owned(command).await?;
        self.supervisor.stop(&run_id).await;
        Ok(accepted)
    }

    pub async fn resolve_interaction(
        &self,
        command: ResolveInteractionCommand,
    ) -> Result<CommandAccepted, DomainError> {
        let interaction_id = command.interaction_id.clone();
        let choice_id = command.choice_id.clone();
        let (accepted, run_id) = self.commands.resolve_interaction_owned(command).await?;
        self.supervisor
            .resolve(&run_id, interaction_id, choice_id)
            .await;
        Ok(accepted)
    }

    pub async fn recover_after_restart(&self) -> Result<(), DomainError> {
        for task_id in self.commands.repository().task_ids()? {
            self.recover_task(&task_id).await?;
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.supervisor.shutdown().await;
    }

    #[cfg(test)]
    pub(super) fn repository(&self) -> &TaskRepository {
        self.commands.repository()
    }

    #[cfg(test)]
    pub(super) fn pause_next_launch(
        &self,
    ) -> (Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>) {
        let arrived = Arc::new(tokio::sync::Barrier::new(2));
        let release = Arc::new(tokio::sync::Barrier::new(2));
        *self
            .supervisor
            .launch_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((arrived.clone(), release.clone()));
        (arrived, release)
    }

    async fn recover_task(&self, task_id: &TaskId) -> Result<(), DomainError> {
        let _task = self.commands.repository().task_guard(task_id).await;
        let aggregate = self.commands.repository().rebuild(task_id)?;
        let projection = aggregate.projection()?;
        let Some(active) = projection.active_run else {
            return Ok(());
        };
        let mut pending_interactions = HashSet::new();
        let mut open_requests = HashMap::<RequestId, RequestScope>::new();
        for record in self.commands.repository().replay(task_id)? {
            let EventPayload::TaskLedger(record) = record.payload else {
                continue;
            };
            let events = match record {
                TaskLedgerRecord::Control(transaction) => transaction.events().to_vec(),
                TaskLedgerRecord::Activity(batch) => batch.events().to_vec(),
            };
            for event in &events {
                match event {
                    TaskEvent::InteractionOpened { interaction }
                        if interaction.run_id == active.run_id =>
                    {
                        pending_interactions.insert(interaction.interaction_id.clone());
                    }
                    TaskEvent::InteractionResolved { interaction_id, .. }
                    | TaskEvent::InteractionCancelled { interaction_id } => {
                        pending_interactions.remove(interaction_id);
                    }
                    TaskEvent::RequestStarted(started)
                        if started.scope.execution.run_id == active.run_id =>
                    {
                        open_requests
                            .insert(started.scope.request_id.clone(), started.scope.clone());
                    }
                    TaskEvent::RequestCompleted(completed) => {
                        open_requests.remove(&completed.scope.request_id);
                    }
                    TaskEvent::RequestFailed(failed) => {
                        open_requests.remove(&failed.scope.request_id);
                    }
                    _ => {}
                }
            }
        }
        let agent_message_id = format!("msg_agent_{}", active.run_id.as_str());
        let mut events = Vec::new();
        if aggregate.message_needs_finalization(&agent_message_id) {
            events.push(TaskEvent::MessagePatched {
                message_id: agent_message_id,
                append_text: String::new(),
                finalized: true,
                request_ids: None,
            });
        }
        events.push(TaskEvent::RunInterrupted {
            run: RunFact {
                run_id: active.run_id,
                task_id: task_id.clone(),
                state: RunState::Interrupted,
                started_at: active.started_at,
                finished_at: Some(super::store::current_timestamp()?),
                summary: Some("server_restarted".to_owned()),
                warnings: Vec::new(),
                checks: None,
                metrics: None,
                workspace_changes: None,
            },
        });
        let mut interactions = pending_interactions.into_iter().collect::<Vec<_>>();
        interactions.sort();
        events.extend(
            interactions
                .into_iter()
                .map(|interaction_id| TaskEvent::InteractionCancelled { interaction_id }),
        );
        let mut requests = open_requests.into_values().collect::<Vec<_>>();
        requests.sort_by(|left, right| left.request_id.cmp(&right.request_id));
        events.extend(requests.into_iter().map(|scope| {
            TaskEvent::RequestFailed(RequestFailed {
                scope,
                usage: None,
                elapsed_ms: None,
                provider_request_id: None,
                cost: None,
                failure: ProviderFailureFact {
                    kind: ProviderFailureKind::ServerRestarted,
                    summary: "server_restarted".to_owned(),
                },
            })
        }));
        let batch = kuku::event::TaskActivityBatch::try_new(events)
            .map_err(|_| DomainError::LedgerCorrupt)?;
        self.commands
            .repository()
            .append(task_id, TaskLedgerRecord::Activity(batch))?;
        Ok(())
    }
}
