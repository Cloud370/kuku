use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::tool::{ToolFaults, ToolResultEnvelope};
use crate::{Error, Result};

use super::record::{timestamp, Record, State};
use super::{
    command_output_id, ensure_open, load_state, Activation, CleanupOutcome, Input, InputSource,
    InputTiming, JobResult, JobSnapshot, JobStatus, Session,
};

pub(super) const JOB_DELIVERY_PREFIX: &str = "job-completion:";
const MAX_JOB_SUMMARY_BYTES: usize = 1024;
const RECENT_JOB_COUNT: usize = 64;

pub(crate) struct JobRequest {
    pub(crate) agent: String,
    pub(crate) execution_id: u64,
    pub(crate) turn: u64,
    pub(crate) tool_call_id: String,
    pub(crate) tool: String,
    pub(crate) summary: String,
    pub(crate) started: Instant,
}

pub(super) struct JobWorker {
    agent: String,
    pub(super) cancel: Arc<Notify>,
    done: watch::Receiver<CleanupOutcome>,
    pub(super) join: JoinHandle<Result<()>>,
}

#[derive(Debug, Default)]
pub(super) struct JobIndex {
    sequence: u64,
    agents: HashMap<String, AgentJobs>,
    active_since: HashMap<u64, Instant>,
}

#[derive(Debug, Default)]
struct AgentJobs {
    running: BTreeSet<u64>,
    jobs: BTreeMap<u64, JobSnapshot>,
}

#[derive(Default)]
pub(super) struct JobReplay {
    calls: HashMap<(String, u64, String), JobCall>,
}

enum JobCall {
    Execution {
        tool: String,
        dispatched: bool,
    },
    Wait {
        job_id: Option<String>,
        dispatched: bool,
    },
}

pub(super) struct JobCompletion<'a> {
    pub(super) event_id: u64,
    pub(super) agent: &'a str,
    pub(super) job_id: &'a str,
    pub(super) result: &'a JobResult,
    pub(super) status: JobStatus,
    pub(super) elapsed_ms: Option<u64>,
}

impl JobReplay {
    pub(super) fn observe(&mut self, state: &mut State, event: &StoredEvent) -> Result<()> {
        match &event.payload {
            EventPayload::ToolCall {
                turn,
                conversation: Some(agent),
                tool_call_id,
                tool,
                ..
            } => {
                let call = match tool.as_str() {
                    "run_command" | "run_js" => JobCall::Execution {
                        tool: tool.clone(),
                        dispatched: false,
                    },
                    "wait_job" => JobCall::Wait {
                        job_id: None,
                        dispatched: false,
                    },
                    _ => return Ok(()),
                };
                self.calls
                    .insert((agent.clone(), *turn, tool_call_id.clone()), call);
            }
            EventPayload::Unknown(data)
                if matches!(
                    data["kind"].as_str(),
                    Some("runtime.tool_prepared" | "runtime.tool_dispatched")
                ) =>
            {
                self.observe_dispatch(data);
            }
            EventPayload::ToolResult {
                turn,
                conversation: Some(agent),
                tool_call_id,
                status,
                structured,
                ..
            } => {
                let call = self
                    .calls
                    .remove(&(agent.clone(), *turn, tool_call_id.clone()));
                if let (Some(data), Some(call)) = (structured, call) {
                    if status == "ok" {
                        self.observe_result(state, event, agent, data, call)?;
                    }
                }
            }
            EventPayload::TurnCompleted {
                turn, conversation, ..
            }
            | EventPayload::TurnCancelled {
                turn, conversation, ..
            }
            | EventPayload::TurnInterrupted {
                turn, conversation, ..
            } => {
                self.calls
                    .retain(|(agent, call_turn, _), _| agent != conversation || call_turn != turn);
            }
            _ => {}
        }
        Ok(())
    }

    fn observe_dispatch(&mut self, data: &serde_json::Value) {
        let (Some(agent), Some(turn), Some(id)) = (
            data["agent"].as_str(),
            data["turn"].as_u64(),
            data["tool_call_id"].as_str(),
        ) else {
            return;
        };
        let Some(call) = self.calls.get_mut(&(agent.into(), turn, id.into())) else {
            return;
        };
        if data["kind"] == "runtime.tool_dispatched" {
            match call {
                JobCall::Execution { dispatched, .. } | JobCall::Wait { dispatched, .. } => {
                    *dispatched = true;
                }
            }
        } else if let JobCall::Wait { job_id, .. } = call {
            *job_id = data["args"]["job_id"].as_str().map(str::to_owned);
        }
    }

    fn observe_result(
        &self,
        state: &mut State,
        event: &StoredEvent,
        agent: &str,
        data: &serde_json::Value,
        call: JobCall,
    ) -> Result<()> {
        match call {
            JobCall::Execution { tool, dispatched } => {
                if dispatched && data["kind"] == "job" && data["tool"] == tool {
                    state.jobs.register(event)?;
                }
            }
            JobCall::Wait { job_id, dispatched } => {
                if let Some(id) = job_id {
                    if dispatched && data["kind"] == "job_wait_result" && data["id"] == id {
                        state.acknowledge_job_result(agent, &id)?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl Session {
    /// Return whether any Session-owned Job is still active.
    pub fn has_active_jobs(&self) -> bool {
        self.inner.state.lock().unwrap().jobs.has_active()
    }

    /// List each active Job's owning Agent and Job identity.
    pub fn running_jobs(&self) -> Vec<(String, String)> {
        self.inner
            .state
            .lock()
            .unwrap()
            .jobs
            .running()
            .map(|job| (job.agent.clone(), job.id.clone()))
            .collect()
    }

    /// List recent Job summaries and all running Jobs owned by one Agent.
    pub fn jobs(&self, agent: &str) -> Result<Vec<JobSnapshot>> {
        let state = self.inner.state.lock().unwrap();
        state.agent(agent)?;
        Ok(state.jobs.summaries(agent))
    }

    pub(crate) fn jobs_before(
        &self,
        agent: &str,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<JobSnapshot>> {
        let state = self.inner.state.lock().unwrap();
        state.agent(agent)?;
        Ok(state.jobs.page(agent, before, limit))
    }

    /// Read one Job's status and terminal facts for its owning Agent.
    pub fn job(&self, agent: &str, job_id: &str) -> Result<JobSnapshot> {
        let snapshot = {
            let state = self.inner.state.lock().unwrap();
            state.agent(agent)?;
            state.jobs.observe_snapshot(state.jobs.get(agent, job_id)?)
        };
        hydrate_job(self.events_path(), snapshot)
    }

    /// Read recent Job summaries from an offline Session without acquiring its runtime lease.
    pub fn read_jobs(events_path: &Path, agent: &str) -> Result<Vec<JobSnapshot>> {
        let (state, _) = load_state(events_path)?;
        state.agent(agent)?;
        Ok(state
            .jobs
            .summaries(agent)
            .into_iter()
            .map(offline_snapshot)
            .collect())
    }

    /// Read one offline Job, marking a missing terminal outcome as interrupted.
    pub fn read_job(events_path: &Path, agent: &str, job_id: &str) -> Result<JobSnapshot> {
        let (state, _) = load_state(events_path)?;
        state.agent(agent)?;
        let snapshot = state.jobs.get(agent, job_id)?.clone();
        hydrate_job(events_path, offline_snapshot(snapshot))
    }

    /// Wait for a Job's terminal state without changing its lifetime.
    pub async fn wait_job(&self, agent: &str, job_id: &str) -> Result<JobSnapshot> {
        let changed = self
            .inner
            .writer
            .lock()
            .unwrap()
            .as_ref()
            .map(|writer| writer.changed());
        let Some(changed) = changed else {
            return self
                .wait_outcome(agent, job_id)?
                .ok_or_else(|| interrupted_job(job_id));
        };
        loop {
            let notified = changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(snapshot) = self.wait_outcome(agent, job_id)? {
                return Ok(snapshot);
            }
            notified.await;
        }
    }

    /// Request cancellation of one owned Job and await its terminal cleanup.
    pub async fn cancel_job(&self, agent: &str, job_id: &str) -> Result<JobSnapshot> {
        let done = {
            let state = self.inner.state.lock().unwrap();
            state.agent(agent)?;
            if state.jobs.get(agent, job_id)?.status == JobStatus::Running {
                ensure_open(&state)?;
                let workers = self.inner.job_workers.lock().unwrap();
                let worker = workers.get(job_id).ok_or_else(|| {
                    Error::InvalidEventStream(format!("job '{job_id}' has no live worker"))
                })?;
                worker.cancel.notify_one();
                Some(worker.done.clone())
            } else {
                None
            }
        };
        if let Some(done) = done {
            await_job_cleanup(done).await?;
        }
        self.job(agent, job_id)
    }

    pub(crate) fn active_jobs(&self, agent: &str) -> usize {
        self.inner.state.lock().unwrap().jobs.running_count(agent)
    }

    pub(crate) fn append_job_wait_result(&self, payload: EventPayload) -> Result<StoredEvent> {
        let EventPayload::ToolResult {
            conversation: Some(agent),
            status,
            structured: Some(data),
            ..
        } = &payload
        else {
            return Err(Error::InvalidEventStream("invalid Job wait result".into()));
        };
        let id = data["id"]
            .as_str()
            .filter(|_| status == "ok" && data["kind"] == "job_wait_result")
            .ok_or_else(|| Error::InvalidEventStream("invalid Job wait identity".into()))?;
        let mut state = self.inner.state.lock().unwrap();
        if state.jobs.get(agent, id)?.status == JobStatus::Running {
            return Err(Error::InvalidEventStream(format!(
                "Job '{id}' returned before completion"
            )));
        }
        let (agent, id) = (agent.clone(), id.to_owned());
        let mut writer = self.inner.writer.lock().unwrap();
        let writer = writer
            .as_mut()
            .ok_or_else(|| Error::InvalidArgument("session resources are closed".into()))?;
        let event = writer.append(payload)?;
        state.acknowledge_job_result(&agent, &id)?;
        Ok(event)
    }

    pub(crate) async fn wait_for_job_input(
        &self,
        agent: &str,
        execution_id: u64,
        turn: u64,
    ) -> Result<()> {
        let changed = self
            .inner
            .writer
            .lock()
            .unwrap()
            .as_ref()
            .map(|writer| writer.changed())
            .ok_or(Error::ExecutionNotActive(execution_id))?;
        loop {
            let notified = changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.inner.state.lock().unwrap();
                let target = self.executing_agent(&state, agent, execution_id)?;
                if target.snapshot.turn != Some(turn) {
                    return Err(Error::ExecutionNotActive(execution_id));
                }
                if let Some(error) = self.inner.settlement_failure.get() {
                    return Err(Error::InvalidEventStream(error.clone()));
                }
                if target
                    .inbox
                    .iter()
                    .any(|delivery| delivery.input.timing == InputTiming::NextStep)
                {
                    return Ok(());
                }
            }
            notified.await;
        }
    }

    pub(crate) fn detach_job<F>(
        &self,
        request: JobRequest,
        cancel: Arc<Notify>,
        completion: impl FnOnce() -> F,
    ) -> Result<ToolResultEnvelope>
    where
        F: Future<Output = ToolResultEnvelope> + Send + 'static,
    {
        let mut state = self.inner.state.lock().unwrap();
        let agent = self.executing_agent(&state, &request.agent, request.execution_id)?;
        if agent.snapshot.turn != Some(request.turn) {
            return Err(Error::ExecutionNotActive(request.execution_id));
        }
        let envelope = self.register_job(&mut state, &request)?;
        let id = job_id(state.jobs.sequence);
        let worker_id = id.clone();
        let worker_agent = request.agent.clone();
        let session = self.clone();
        let started = request.started;
        let (complete, done) = watch::channel(None);
        let completion = completion();
        let join = tokio::spawn(async move {
            let outcome = match tokio::spawn(completion).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    unconfirmed_result(format!("Job worker ended before confirmation: {error}"))
                }
            };
            let result =
                session.finish_job(&worker_agent, &worker_id, outcome, elapsed_ms(started));
            let _ = complete.send(Some(
                result.as_ref().map(|_| ()).map_err(ToString::to_string),
            ));
            result
        });
        let mut workers = self.inner.job_workers.lock().unwrap();
        workers.retain(|_, worker| !worker.join.is_finished());
        workers.insert(
            id,
            JobWorker {
                agent: request.agent,
                cancel,
                done,
                join,
            },
        );
        Ok(envelope)
    }

    pub(super) fn recover_jobs(&self) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        for job in state.jobs.unsettled() {
            let summary = "Job interrupted before a durable terminal outcome; inspect side effects before rerunning";
            let result = JobResult {
                status: "interrupted".into(),
                summary: summary.into(),
                structured: None,
                truncated: false,
                output_persistence_failed: false,
                cleanup_incomplete: true,
            };
            let delivery = self.job_delivery(&job, JobStatus::Interrupted, &result, "");
            self.commit(
                &mut state,
                Record::JobFinished {
                    job_id: job.id,
                    agent: job.agent,
                    turn: job.turn,
                    result,
                    status: JobStatus::Interrupted,
                    elapsed_ms: None,
                    delivery: Box::new(delivery),
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn cancel_owned_jobs(&self, agent: &str) -> Vec<watch::Receiver<CleanupOutcome>> {
        self.inner
            .job_workers
            .lock()
            .unwrap()
            .values()
            .filter(|worker| worker.agent == agent && !worker.join.is_finished())
            .map(|worker| {
                worker.cancel.notify_one();
                worker.done.clone()
            })
            .collect()
    }

    fn register_job(&self, state: &mut State, request: &JobRequest) -> Result<ToolResultEnvelope> {
        let sequence = state
            .jobs
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::InvalidEventStream("Job identities exhausted".into()))?;
        let id = job_id(sequence);
        let output_id = (request.tool == "run_command")
            .then(|| command_output_id(&request.agent, request.turn, &request.tool_call_id));
        let title = bounded(&request.summary, MAX_JOB_SUMMARY_BYTES);
        let summary = format!("{id} running: {title}");
        let facts = json!({
            "kind": "job",
            "job_id": id,
            "status": JobStatus::Running,
            "tool": request.tool,
            "title": title,
            "elapsed_ms": elapsed_ms(request.started),
            "output_id": output_id,
        });
        let envelope = ToolResultEnvelope::ok(&summary, serde_json::to_string(&facts)?, facts);
        let mut writer = self.inner.writer.lock().unwrap();
        let writer = writer
            .as_mut()
            .ok_or(Error::ExecutionNotActive(request.execution_id))?;
        let event = writer.append(EventPayload::ToolResult {
            turn: request.turn,
            ts: timestamp()?,
            conversation: Some(request.agent.clone()),
            tool_call_id: request.tool_call_id.clone(),
            status: envelope.status.clone(),
            summary,
            model_content: envelope.model_content.clone(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: envelope.structured.clone(),
        })?;
        state.jobs.register(&event)?;
        state.jobs.active_since.insert(sequence, request.started);
        Ok(envelope)
    }

    fn finish_job(
        &self,
        agent: &str,
        job_id: &str,
        outcome: ToolResultEnvelope,
        elapsed_ms: u64,
    ) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        let job = state.jobs.get(agent, job_id)?;
        let status = JobStatus::for_result(&outcome);
        let failure =
            if outcome.faults.output_persistence_failed || outcome.faults.cleanup_incomplete {
                Some(
                    self.remember_settlement_failure(Error::InvalidEventStream(format!(
                        "job '{job_id}' could not confirm output persistence or cleanup: {}",
                        outcome.summary
                    ))),
                )
            } else {
                None
            };
        let result = JobResult {
            status: outcome.status,
            summary: outcome.summary,
            structured: outcome.structured,
            truncated: outcome.truncated,
            output_persistence_failed: outcome.faults.output_persistence_failed,
            cleanup_incomplete: outcome.faults.cleanup_incomplete,
        };
        let delivery = self.job_delivery(job, status, &result, &outcome.model_content);
        let turn = job.turn;
        if let Err(error) = self.commit(
            &mut state,
            Record::JobFinished {
                job_id: job_id.into(),
                agent: agent.into(),
                turn,
                result,
                status,
                elapsed_ms: Some(elapsed_ms),
                delivery: Box::new(delivery),
            },
        ) {
            let error = self.remember_settlement_failure(error);
            if let Some(writer) = self.inner.writer.lock().unwrap().as_ref() {
                writer.changed().notify_waiters();
            }
            return Err(error);
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if !state.closing {
            self.schedule_locked(&mut state, agent)?;
        }
        Ok(())
    }

    fn job_delivery(
        &self,
        job: &JobSnapshot,
        status: JobStatus,
        result: &JobResult,
        model_content: &str,
    ) -> Input {
        let mut text = format!(
            "Job {} ({}): {status:?}. {}",
            job.id, job.tool, result.summary
        );
        let body = model_content
            .strip_prefix(result.summary.as_str())
            .and_then(|rest| {
                rest.strip_prefix('\n')
                    .or_else(|| rest.is_empty().then_some(rest))
            })
            .unwrap_or(model_content);
        if !body.is_empty() {
            text.push('\n');
            text.push_str(body);
        }
        let limit = self
            .inner
            .options
            .limits
            .max_input_bytes
            .min(self.inner.options.limits.max_preview_bytes);
        if text.len() > limit {
            let mut suffix = "\n\nJob preview truncated.".to_string();
            if let Some(id) = result.output_id() {
                suffix.push_str(&format!("\nOutput: {id} (read more with read_output)."));
            }
            let suffix = bounded(&suffix, limit);
            text.truncate(text.floor_char_boundary(limit - suffix.len()));
            text.push_str(suffix);
        }
        Input {
            command_id: format!("{JOB_DELIVERY_PREFIX}{}", job.id),
            source: InputSource::session_job(),
            target: job.agent.clone(),
            text,
            activation: Activation::Wake,
            correlation: Some(job.id.clone()),
            timing: InputTiming::NextStep,
            bootstrap_skill: None,
        }
    }

    fn wait_outcome(&self, agent: &str, job_id: &str) -> Result<Option<JobSnapshot>> {
        let (snapshot, closed) = {
            let state = self.inner.state.lock().unwrap();
            state.agent(agent)?;
            (state.jobs.get(agent, job_id)?.clone(), state.closed)
        };
        if snapshot.status != JobStatus::Running {
            return hydrate_job(self.events_path(), snapshot).map(Some);
        }
        if let Some(error) = self.inner.settlement_failure.get() {
            return Err(Error::InvalidEventStream(error.clone()));
        }
        if closed {
            return Err(interrupted_job(job_id));
        }
        Ok(None)
    }
}

impl JobIndex {
    pub(super) fn running_count(&self, agent: &str) -> usize {
        self.agents.get(agent).map_or(0, |jobs| jobs.running.len())
    }

    fn register(&mut self, event: &StoredEvent) -> Result<()> {
        let EventPayload::ToolResult {
            turn,
            conversation: Some(agent),
            tool_call_id,
            summary,
            structured: Some(data),
            ..
        } = &event.payload
        else {
            return Ok(());
        };
        if data["kind"] != "job" {
            return Ok(());
        }
        let invalid =
            || Error::InvalidEventStream(format!("invalid Job registration at event {}", event.id));
        let id = data["job_id"].as_str().ok_or_else(invalid)?;
        let sequence = job_sequence(id).ok_or_else(invalid)?;
        let tool = data["tool"].as_str().ok_or_else(invalid)?;
        if self.sequence.checked_add(1) != Some(sequence) {
            return Err(invalid());
        }
        self.sequence = sequence;
        let agent_jobs = self.agents.entry(agent.clone()).or_default();
        agent_jobs.running.insert(sequence);
        agent_jobs.jobs.insert(
            sequence,
            JobSnapshot {
                id: id.into(),
                agent: agent.clone(),
                turn: *turn,
                tool_call_id: tool_call_id.clone(),
                tool: tool.into(),
                status: JobStatus::Running,
                title: bounded(
                    data["title"].as_str().unwrap_or(tool),
                    MAX_JOB_SUMMARY_BYTES,
                )
                .into(),
                summary: bounded(summary, MAX_JOB_SUMMARY_BYTES).into(),
                elapsed_ms: None,
                output_id: data["output_id"].as_str().map(str::to_owned),
                started_at: event.id,
                finished_at: None,
                result: None,
            },
        );
        Ok(())
    }

    pub(super) fn finish(&mut self, completion: JobCompletion<'_>) -> Result<()> {
        let error = || {
            Error::InvalidEventStream(format!(
                "invalid terminal event for job '{}'",
                completion.job_id
            ))
        };
        let jobs = self.agents.get_mut(completion.agent).ok_or_else(error)?;
        let sequence = job_sequence(completion.job_id).ok_or_else(error)?;
        let snapshot = jobs.jobs.get_mut(&sequence).ok_or_else(error)?;
        if snapshot.status != JobStatus::Running || completion.status == JobStatus::Running {
            return Err(error());
        }
        snapshot.status = completion.status;
        snapshot.elapsed_ms = completion.elapsed_ms;
        snapshot.summary = bounded(&completion.result.summary, MAX_JOB_SUMMARY_BYTES).into();
        snapshot.output_id = completion.result.output_id().map(str::to_owned);
        snapshot.finished_at = Some(completion.event_id);
        jobs.running.remove(&sequence);
        self.active_since.remove(&sequence);
        Ok(())
    }

    fn get(&self, agent: &str, id: &str) -> Result<&JobSnapshot> {
        job_sequence(id)
            .and_then(|sequence| self.agents.get(agent)?.jobs.get(&sequence))
            .filter(|job| job.id == id)
            .ok_or_else(|| Error::InvalidArgument(format!("no job '{id}' for agent '{agent}'")))
    }

    fn summaries(&self, agent: &str) -> Vec<JobSnapshot> {
        let Some(jobs) = self.agents.get(agent) else {
            return Vec::new();
        };
        let selected = jobs
            .jobs
            .keys()
            .rev()
            .take(RECENT_JOB_COUNT)
            .chain(&jobs.running)
            .collect::<BTreeSet<_>>();
        selected
            .into_iter()
            .rev()
            .map(|sequence| self.observe_snapshot(&jobs.jobs[sequence]))
            .collect()
    }

    fn page(&self, agent: &str, before: Option<u64>, limit: usize) -> Vec<JobSnapshot> {
        self.agents
            .get(agent)
            .into_iter()
            .flat_map(|jobs| jobs.jobs.values().rev())
            .skip_while(|job| before.is_some_and(|before| job.started_at >= before))
            .take(limit)
            .map(|snapshot| self.observe_snapshot(snapshot))
            .collect()
    }

    fn observe_snapshot(&self, snapshot: &JobSnapshot) -> JobSnapshot {
        let mut snapshot = snapshot.clone();
        if let Some(started) =
            job_sequence(&snapshot.id).and_then(|sequence| self.active_since.get(&sequence))
        {
            snapshot.elapsed_ms = Some(elapsed_ms(*started));
        }
        snapshot
    }

    fn unsettled(&self) -> Vec<JobSnapshot> {
        let mut jobs = self.running().cloned().collect::<Vec<_>>();
        jobs.sort_unstable_by_key(|job| job.started_at);
        jobs
    }

    pub(super) fn capture_active(&self, agent: &str, turn: u64, tool_call_id: &str) -> bool {
        self.agents.get(agent).is_some_and(|jobs| {
            jobs.running.iter().any(|sequence| {
                let job = &jobs.jobs[sequence];
                job.turn == turn && job.tool_call_id == tool_call_id
            })
        })
    }

    pub(super) fn has_active(&self) -> bool {
        self.agents.values().any(|jobs| !jobs.running.is_empty())
    }

    fn running(&self) -> impl Iterator<Item = &JobSnapshot> {
        self.agents
            .values()
            .flat_map(|jobs| jobs.running.iter().map(|sequence| &jobs.jobs[sequence]))
    }
}

pub(super) async fn await_job_cleanup(mut done: watch::Receiver<CleanupOutcome>) -> Result<()> {
    loop {
        if let Some(result) = done.borrow().clone() {
            return result.map_err(Error::InvalidEventStream);
        }
        done.changed().await.map_err(|_| {
            Error::InvalidEventStream("Job cleanup task ended before a terminal result".into())
        })?;
    }
}

fn job_id(sequence: u64) -> String {
    format!("job-{sequence}")
}

fn job_sequence(id: &str) -> Option<u64> {
    id.strip_prefix("job-")?.parse().ok()
}

fn missing_job_result(id: &str) -> Error {
    Error::InvalidEventStream(format!(
        "terminal state for job '{id}' is missing its result"
    ))
}

fn interrupted_job(id: &str) -> Error {
    Error::InterruptedOpenTool(format!(
        "job '{id}' has no confirmed result after the Session writer closed"
    ))
}

fn hydrate_job(events_path: &Path, mut snapshot: JobSnapshot) -> Result<JobSnapshot> {
    if let Some(cursor) = snapshot.finished_at {
        let events = EventStore::read_page(events_path, cursor - 1, 1)?;
        let event = events
            .first()
            .filter(|event| event.id == cursor)
            .ok_or_else(|| missing_job_result(&snapshot.id))?;
        let Some(Record::JobFinished { result, .. }) = Record::from_event(event)? else {
            return Err(missing_job_result(&snapshot.id));
        };
        snapshot.result = Some(result);
    }
    Ok(snapshot)
}

fn offline_snapshot(mut snapshot: JobSnapshot) -> JobSnapshot {
    if snapshot.status == JobStatus::Running {
        snapshot.status = JobStatus::Interrupted;
        snapshot.summary = "Job outcome was unconfirmed when the Session stopped".into();
        snapshot.output_id = None;
    }
    snapshot
}

fn bounded(text: &str, max_bytes: usize) -> &str {
    &text[..text.floor_char_boundary(text.len().min(max_bytes))]
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn unconfirmed_result(summary: String) -> ToolResultEnvelope {
    ToolResultEnvelope {
        faults: ToolFaults {
            cleanup_incomplete: true,
            ..ToolFaults::default()
        },
        ..ToolResultEnvelope::error(&summary, &summary)
    }
}

#[cfg(test)]
mod tests;
