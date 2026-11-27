//! Follow the receipt's admission and its Turn through the resumable Session stream.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use kuku::event::EventPayload;
use kuku::session::runtime::ModelRequestRoute;
use kuku_client::{
    Acknowledgement, Action, AgentConfig, AgentSettlement, AgentSnapshot, AgentStatus, Command,
    Connection, HistoryKind, InputDelivery, InputStatus, JobSnapshot, JobStatus, Observation,
    PendingApproval, PermissionRequest, QuestionRequest, Snapshot, Target, UsageTotals, new_id,
};
use serde::Serialize;
use serde_json::json;

use super::render::{RunRenderer, write_json};
use super::{RunOutcome, execution_not_active, request_cancel, submit};
use crate::commands::CliResult;

type Error = Box<dyn std::error::Error>;

#[derive(Serialize)]
pub(super) struct RunResult {
    pub(super) agent_status: AgentStatus,
    #[serde(rename = "turns")]
    turn: Option<u64>,
    pub(super) response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    pub(super) permission: Option<PermissionRequest>,
    pub(super) question: Option<QuestionRequest>,
    config: Option<AgentConfig>,
    pub(super) last_model_route: Option<ModelRequestRoute>,
    usage: Option<UsageTotals>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_status: Option<InputStatus>,
    #[serde(skip)]
    job: Option<JobSnapshot>,
    #[serde(skip)]
    current_agent_state: bool,
}

impl RunResult {
    pub(super) fn outcome(&self) -> RunOutcome {
        match self.agent_status {
            _ if self.permission.is_some() || self.question.is_some() => RunOutcome::Waiting,
            AgentStatus::Waiting => RunOutcome::Waiting,
            _ if self.error.is_some() => RunOutcome::Failed,
            AgentStatus::Idle => RunOutcome::Completed,
            AgentStatus::Stopped => RunOutcome::Cancelled,
            AgentStatus::Interrupted | AgentStatus::Running => RunOutcome::Failed,
        }
    }

    pub(super) fn waiting(agent: &AgentSnapshot) -> Self {
        Self {
            agent_status: agent.status,
            turn: agent.turn,
            response: None,
            error: None,
            question: agent.question.clone(),
            permission: agent.permission.clone(),
            config: None,
            last_model_route: None,
            usage: None,
            input_status: None,
            job: None,
            current_agent_state: false,
        }
    }

    pub(super) fn finish_reply(self, agent: &AgentSnapshot) -> CliResult {
        let outcome = self.outcome();
        let mut record = if self.current_agent_state && self.agent_status == agent.status {
            serde_json::to_value(agent)?
        } else {
            json!({"address":agent.address,"config":self.config,"usage":self.usage,"last_model_route":self.last_model_route})
        };
        record["status"] = json!(self.agent_status);
        record["turn"] = json!(self.turn);
        record["last_output"] = json!(self.response);
        record["last_error"] = json!(self.error);
        record["question"] = json!(self.question);
        record["permission"] = json!(self.permission);
        if let Some(job) = self.job {
            record["job"] = json!(job);
        }
        write_json(&record)?;
        match outcome {
            RunOutcome::Completed | RunOutcome::Waiting => Ok(()),
            _ => Err(self.error.unwrap_or_else(|| outcome.to_string()).into()),
        }
    }

    fn from_job(job: JobSnapshot, permission: Option<PermissionRequest>) -> Self {
        let status = match job.status {
            JobStatus::Running => AgentStatus::Waiting,
            JobStatus::Completed => AgentStatus::Idle,
            JobStatus::Cancelled => AgentStatus::Stopped,
            JobStatus::Failed | JobStatus::Interrupted => AgentStatus::Interrupted,
        };
        Self {
            agent_status: status,
            turn: Some(job.turn),
            response: None,
            error: (status == AgentStatus::Interrupted).then(|| job.summary.clone()),
            permission,
            question: None,
            config: None,
            last_model_route: None,
            usage: None,
            input_status: None,
            job: Some(job),
            current_agent_state: false,
        }
    }
}

enum Boundary {
    Settlement(AgentSettlement),
    Permission(PermissionRequest),
}

#[derive(Default)]
struct InputObservation {
    /// (admission cursor, Turn). A steering admission can join an earlier Turn.
    admission: Option<(u64, u64)>,
    question: Option<QuestionRequest>,
    settlement: Option<(u64, Boundary)>,
    later_agent_change: bool,
    config_changed: bool,
    awaiting_permission_resume: bool,
    dispatched: bool,
}

impl InputObservation {
    fn owns(&self, turn: Option<u64>) -> bool {
        self.admission
            .is_some_and(|(_, admitted)| Some(admitted) == turn)
    }

    fn observe(
        &mut self,
        frame: Observation,
        target: &Target,
        input: &str,
        renderer: Option<&mut RunRenderer>,
    ) -> CliResult {
        match frame {
            Observation::Committed {
                id,
                agent,
                kind,
                turn,
                delivery,
                settlement,
                entry,
                state_changed,
                operation,
                ..
            } if agent.as_deref() == Some(&target.agent) => {
                match delivery.as_deref() {
                    Some(InputDelivery::Accepted { input_ids })
                        if self.admission.is_none() && input_ids.iter().any(|id| id == input) =>
                    {
                        self.admission = Some((id, turn.ok_or("input admission has no Turn")?));
                    }
                    Some(
                        InputDelivery::Withdrawn { input_id }
                        | InputDelivery::Replaced { input_id, .. },
                    ) if input_id == input => return Err(RunOutcome::InputWithdrawn.into()),
                    _ => {}
                }
                if self.settlement.is_some() {
                    self.later_agent_change |= state_changed;
                    return Ok(());
                }
                if self.awaiting_permission_resume
                    && operation.as_deref() == Some("execution_started")
                    && let Some((cursor, _)) = &mut self.admission
                {
                    *cursor = id;
                    self.awaiting_permission_resume = false;
                }
                if self.admission.is_some() {
                    match operation.as_deref() {
                        Some("agent_configured" | "input_delivered") => self.config_changed = true,
                        Some("step_configured") => self.config_changed = false,
                        _ => {}
                    }
                }
                if self.owns(turn)
                    && matches!(kind.as_str(), "runtime.model_dispatched" | "model.response")
                {
                    self.dispatched = true;
                }
                if let Some(entry) = entry.filter(|entry| self.owns(entry.turn)) {
                    match entry.kind {
                        HistoryKind::Question => {
                            self.question =
                                Some(serde_json::from_value(entry.details["request"].clone())?);
                        }
                        HistoryKind::Permission
                            if operation.as_deref() == Some("permission_waiting") =>
                        {
                            // Native approval ends this execution. A reply can resume the same Turn
                            // in a different execution before a delayed observer reaches this frame.
                            let request = serde_json::from_value(entry.details["request"].clone())?;
                            self.settlement = Some((id, Boundary::Permission(request)));
                        }
                        _ => {}
                    }
                    if let Some(renderer) = renderer {
                        renderer.committed(&entry)?;
                    }
                }
                if let Some(settlement) = settlement.filter(|end| self.owns(end.turn)) {
                    self.settlement = Some((id, Boundary::Settlement(*settlement)));
                }
            }
            Observation::Live {
                agent,
                turn,
                cursor,
                event,
                ..
            } if agent == target.agent
                && self.owns(turn)
                && self
                    .admission
                    .is_some_and(|(admission, _)| cursor >= admission)
                && self
                    .settlement
                    .as_ref()
                    .is_none_or(|(end, _)| cursor <= *end)
                && let Some(renderer) = renderer =>
            {
                renderer.event(event)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self, snapshot: &Snapshot) -> Option<RunResult> {
        let (cursor, end) = self.settlement.take()?;
        // A current snapshot can supplement this settlement only when it still describes this Turn.
        let metadata = (self.owns(snapshot.agent.turn)
            && (snapshot.cursor <= cursor || !self.later_agent_change))
            .then_some(&snapshot.agent);
        let (status, turn, output, error, usage, permission) = match end {
            Boundary::Settlement(end) => {
                (end.status, end.turn, end.output, end.error, end.usage, None)
            }
            Boundary::Permission(request) => (
                AgentStatus::Waiting,
                Some(request.turn),
                None,
                None,
                None,
                Some(request),
            ),
        };
        let waiting = status == AgentStatus::Waiting;
        let mut result = RunResult {
            agent_status: status,
            turn,
            response: if status == AgentStatus::Idle {
                output
            } else {
                None
            },
            error,
            question: if waiting {
                self.question.take().or_else(|| metadata?.question.clone())
            } else {
                None
            },
            permission,
            config: metadata.filter(|_| !self.config_changed).map(|agent| {
                agent
                    .active_config
                    .as_ref()
                    .unwrap_or(&agent.config)
                    .clone()
            }),
            last_model_route: metadata
                .filter(|_| self.dispatched)
                .and_then(|agent| agent.last_model_route.clone()),
            usage,
            input_status: Some(InputStatus::Accepted),
            job: None,
            current_agent_state: metadata.is_some() && !self.config_changed && self.dispatched,
        };
        if result.outcome() == RunOutcome::Failed && result.error.is_none() {
            result.error = Some(RunOutcome::Failed.to_string());
        }
        Some(result)
    }

    fn blocked(
        &self,
        snapshot: &Snapshot,
        approvals: &[PendingApproval],
        input: &str,
    ) -> Option<RunResult> {
        let agent = &snapshot.agent;
        let queued = self.admission.is_none()
            && snapshot
                .pending
                .iter()
                .any(|item| item.receipt.command_id == input);
        if (!queued && !self.owns(agent.turn)) || snapshot.cleaning_up {
            return None;
        }
        let script = approvals.iter().find(|approval| {
            let request = &approval.request;
            request.conversation.as_str() == agent.address
                && request.id.starts_with("script:")
                && Some(request.turn) == agent.turn
                && (!queued || approval.job_id.is_none())
        });
        let mut result = RunResult::waiting(agent);
        result.current_agent_state = self.owns(agent.turn)
            && !self.config_changed
            && self.dispatched
            && !self.later_agent_change;
        result.input_status = Some(if queued {
            InputStatus::Queued
        } else {
            InputStatus::Accepted
        });
        if queued {
            result.turn = None;
        }
        if let Some(job_id) = agent.job_needing_review().filter(|_| queued) {
            result.error = Some(format!(
                "Input saved in the queue. Review the unconfirmed cleanup of {job_id} in /jobs."
            ));
        } else if let Some(approval) = script {
            result.permission = Some(approval.request.clone());
        } else if matches!(
            agent.status,
            AgentStatus::Stopped | AgentStatus::Interrupted
        ) {
            if queued {
                result.agent_status = AgentStatus::Interrupted;
                result.error =
                    Some("The input remains queued. Continue the Agent to admit it.".into());
            } else {
                // A crashed service can recover an interrupted execution without a TurnSettled frame.
                result.error.clone_from(&agent.last_error);
                if result.outcome() == RunOutcome::Failed && result.error.is_none() {
                    result.error = Some(RunOutcome::Failed.to_string());
                }
            }
        } else if agent.status != AgentStatus::Waiting {
            return None;
        }
        Some(result)
    }
}

pub(super) async fn observe_command(
    connection: &Connection,
    command: &Command,
    acknowledgement: &Acknowledgement,
    replied_to: Option<&PendingApproval>,
    mut renderer: Option<&mut RunRenderer>,
    mut interrupt: Pin<&mut impl Future<Output = ()>>,
) -> Result<(RunResult, AgentSnapshot), Error> {
    let target = &command.target;
    if acknowledgement.status == Some(InputStatus::Withdrawn) {
        return Err(RunOutcome::InputWithdrawn.into());
    }
    let receipt = &acknowledgement.receipt;
    let input = receipt.command_id.as_str();
    let mut observer = connection
        .client
        .subscribe_session(target, receipt.event_id.saturating_sub(1));
    let mut snapshot = connection.client.snapshot(target).await?;
    let mut approvals = connection.client.pending_approvals(target).await?;
    let script = replied_to.filter(|approval| approval.request.id.starts_with("script:"));
    let mut job = match script.and_then(|approval| approval.job_id.as_deref()) {
        Some(id) => Some(connection.client.job(target, id).await?),
        None => None,
    };
    let mut checked_jobs = None;
    let mut observed = InputObservation {
        admission: replied_to.map(|approval| (receipt.event_id, approval.request.turn)),
        awaiting_permission_resume: replied_to.is_some() && script.is_none(),
        ..Default::default()
    };
    let mut interrupted = false;
    let mut cancel_sent = false;
    loop {
        let caught_up = observer.cursor() >= snapshot.cursor;
        if job.is_none()
            && checked_jobs != Some(snapshot.cursor)
            && let Some(approval) = script
            && (snapshot.agent.running_jobs > 0
                || observed.settlement.is_some()
                || interrupted
                || snapshot.agent.turn != Some(approval.request.turn))
        {
            // A foreground script can become a Job after the request was inspected.
            job = connection
                .client
                .jobs(target)
                .await?
                .into_iter()
                .find(|job| {
                    job.turn == approval.request.turn
                        && job.tool_call_id == approval.request.tool_call_id
                });
            checked_jobs = Some(snapshot.cursor);
        }
        if let Some(current) = &job {
            let permission = approvals.iter().find(|approval| {
                approval.request.conversation.as_str() == target.agent
                    && approval.request.turn == current.turn
                    && approval.request.tool_call_id == current.tool_call_id
            });
            if permission.is_some() || current.status != JobStatus::Running {
                let result = RunResult::from_job(
                    current.clone(),
                    permission.map(|approval| approval.request.clone()),
                );
                return Ok((result, snapshot.agent));
            }
            if interrupted && !cancel_sent {
                job = Some(connection.client.cancel_job(target, &current.id).await?);
                cancel_sent = true;
                continue;
            }
        } else if caught_up {
            if let Some(blocked) = observed.blocked(&snapshot, &approvals, input)
                && (observed.settlement.is_none()
                    || (blocked.permission.is_some()
                        && !observed.later_agent_change
                        && matches!(&observed.settlement,
                        Some((_, Boundary::Settlement(end))) if end.status == AgentStatus::Idle)))
            {
                return Ok((blocked, snapshot.agent));
            }
            if (interrupted || !super::running_peers(&snapshot, &approvals))
                && !(observed.owns(snapshot.agent.turn) && snapshot.cleaning_up)
                && observed.settlement.is_some()
            {
                if let Some(renderer) = renderer.as_deref_mut().filter(|r| r.needs_preview()) {
                    let drain = async {
                        for _ in 0..256 {
                            let Ok(frame) = observer.next().await else {
                                break;
                            };
                            if let Observation::Live { .. } = frame {
                                observed.observe(frame, target, input, Some(renderer))?;
                            }
                        }
                        CliResult::Ok(())
                    };
                    if let Ok(result) = tokio::time::timeout(Duration::from_millis(20), drain).await
                    {
                        result?;
                    }
                }
                let result = observed
                    .finish(&snapshot)
                    .expect("the Turn has a settlement");
                return Ok((result, snapshot.agent));
            }
        }
        if interrupted
            && !cancel_sent
            && job.is_none()
            && !observed.awaiting_permission_resume
            && observed.settlement.is_none()
            && let Some((admission, turn)) = observed.admission
            && snapshot.agent.turn == Some(turn)
            && let Some(execution) = snapshot.execution_id().filter(|id| *id <= admission)
        {
            if let Err(error) = request_cancel(connection, target, execution).await
                && !execution_not_active(error.as_ref())
            {
                return Err(error);
            }
            cancel_sent = true;
            snapshot = connection.client.snapshot(target).await?;
            continue;
        }
        let mut frame = tokio::select! {
            biased;
            () = &mut interrupt, if !interrupted => {
                interrupted = true;
                if observed.admission.is_none() && matches!(&command.action, Action::Send { .. }) {
                    let withdrawal = submit(connection, &Command {
                        target: target.clone(),
                        command_id: new_id(),
                        action: Action::Withdraw { input_command_id: input.to_owned(), draft: None },
                    }).await?;
                    match withdrawal.status {
                        Some(InputStatus::Withdrawn) => return Err(RunOutcome::InputWithdrawn.into()),
                        Some(InputStatus::Accepted) => {}
                        _ => return Err("service did not report the disposition of the submitted input".into()),
                    }
                }
                snapshot = connection.client.snapshot(target).await?;
                continue;
            }
            frame = observer.next() => frame?,
        };
        // Earlier protocol-v2 services omit the Turn on steering admissions. Read just
        // that committed event instead of guessing from a snapshot that may have advanced.
        if let Observation::Committed {
            id,
            agent,
            turn,
            delivery,
            ..
        } = &mut frame
            && turn.is_none()
            && agent.as_deref() == Some(&target.agent)
            && matches!(delivery.as_deref(), Some(InputDelivery::Accepted { input_ids }) if input_ids.iter().any(|key| key == input))
        {
            let page = connection
                .client
                .events(
                    &target.workspace,
                    &target.session_id,
                    Some(&target.agent),
                    id.saturating_sub(1),
                    1,
                )
                .await?;
            if let Some(event) = page.events.first().filter(|event| event.id == *id)
                && let EventPayload::Unknown(payload) = &event.payload
                && payload["record"]["operation"] == "inputs_accepted"
            {
                *turn = Some(payload["record"]["turn"].as_u64().unwrap_or(*id));
            }
        }
        match &frame {
            Observation::Committed {
                id,
                state_changed: true,
                operation,
                ..
            } => {
                if *id > snapshot.cursor {
                    snapshot = connection.client.snapshot(target).await?;
                }
                if operation.as_deref() == Some("job_finished")
                    && let Some(current) = &mut job
                {
                    *current = connection.client.job(target, &current.id).await?;
                }
                if matches!(
                    operation.as_deref(),
                    Some(
                        "script_permission_waiting"
                            | "script_permission_replied"
                            | "script_permission_ended"
                    )
                ) {
                    approvals = connection.client.pending_approvals(target).await?;
                }
            }
            Observation::Lagged { skipped } => {
                if renderer.as_ref().is_some_and(|r| r.streaming()) {
                    write_json(&json!({"type":"lagged","skipped":skipped}))?;
                }
                snapshot = connection.client.snapshot(target).await?;
                approvals = connection.client.pending_approvals(target).await?;
                if let Some(current) = &mut job {
                    *current = connection.client.job(target, &current.id).await?;
                }
            }
            _ => {}
        }
        observed.observe(frame, target, input, renderer.as_deref_mut())?;
    }
}
