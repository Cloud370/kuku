//! Script submission, exact replies, and observation through the Session service.

use std::fmt;
use std::future::Future;
use std::io::{self, Read};
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, Instant};

use kuku::event::EventPayload;
use kuku_client::{
    Acknowledgement, Action, AgentConfig, AgentSnapshot, AgentStatus, Command, Connection, Error,
    InputDelivery, InputStatus, JobSnapshot, JobStatus, Observation, PendingApproval,
    PermissionRequest, Receipt, Snapshot, Target, Timing, new_id,
};
use serde_json::json;

use crate::cli_args::{ConnectionArgs, ReplyArgs, RunArgs};
use crate::commands::CliResult;
use crate::display::OutputLine;

mod render;

use render::{RunRenderer, write_json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunOutcome {
    Completed,
    Waiting,
    Failed,
    Cancelled,
    CancelledBeforeSubmission,
    InputWithdrawn,
}

impl RunOutcome {
    fn from_snapshot(agent: &AgentSnapshot) -> Self {
        match agent.status {
            _ if agent.permission.is_some() || agent.question.is_some() => Self::Waiting,
            AgentStatus::Waiting => Self::Waiting,
            _ if agent.last_error.is_some() => Self::Failed,
            AgentStatus::Idle => Self::Completed,
            AgentStatus::Stopped => Self::Cancelled,
            AgentStatus::Interrupted | AgentStatus::Running => Self::Failed,
        }
    }

    pub(super) fn exit_status(self) -> i32 {
        match self {
            Self::Completed => 0,
            Self::Waiting => 3,
            Self::Failed => 1,
            Self::Cancelled | Self::CancelledBeforeSubmission | Self::InputWithdrawn => 130,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
            Self::Cancelled | Self::CancelledBeforeSubmission | Self::InputWithdrawn => "cancelled",
        }
    }
}

impl fmt::Display for RunOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Completed => "Agent completed",
            Self::Waiting => "Agent is waiting. Use `kuku reply` with the pending request ID",
            Self::Failed => "Agent execution was interrupted before completion",
            Self::Cancelled => "Agent cancelled after cleanup",
            Self::CancelledBeforeSubmission => "Run cancelled before submitting input",
            Self::InputWithdrawn => "Submitted input withdrawn from the Agent queue",
        })
    }
}

impl std::error::Error for RunOutcome {}

struct RunProgress {
    session_id: Option<String>,
    acknowledgement: Option<Acknowledgement>,
}

/// Execute one script command, then render a terminal run result.
pub async fn run(args: RunArgs, options: &ConnectionArgs) -> CliResult {
    let mut renderer = RunRenderer::new(&args);
    let start = Instant::now();
    let mut progress = RunProgress {
        session_id: args.session.clone(),
        acknowledgement: None,
    };
    let snapshot = match execute_run(&args, options, &mut renderer, &mut progress).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return Ok(()),
        Err(error) => {
            renderer.finish_error(&progress, &args.agent, error.as_ref(), start.elapsed())?;
            return Err(error);
        }
    };
    let outcome = RunOutcome::from_snapshot(&snapshot);
    renderer.finish(&progress, &snapshot, outcome, start.elapsed())?;
    match outcome {
        RunOutcome::Completed => Ok(()),
        RunOutcome::Failed => Err(snapshot
            .last_error
            .unwrap_or_else(|| outcome.to_string())
            .into()),
        outcome => Err(outcome.into()),
    }
}

async fn execute_run(
    args: &RunArgs,
    options: &ConnectionArgs,
    renderer: &mut RunRenderer,
    progress: &mut RunProgress,
) -> Result<Option<AgentSnapshot>, Box<dyn std::error::Error>> {
    let (text, config) = run_input(args)?;
    let connection = options.connect().await?;
    let mut interrupt = std::pin::pin!(ctrl_c_listener()?);
    let preflight = async {
        let (command, recovered) = prepare_run(args, &connection, text, config).await?;
        progress.session_id = Some(command.target.session_id.clone());
        let waiting = if recovered.is_none() {
            preflight_send(&connection, &command).await?
        } else {
            None
        };
        Ok::<_, Box<dyn std::error::Error>>((command, recovered, waiting))
    };
    let (command, recovered, waiting) = tokio::select! {
        biased;
        () = &mut interrupt => return Err(RunOutcome::CancelledBeforeSubmission.into()),
        result = preflight => result?,
    };
    if let Some(waiting) = waiting {
        return Ok(Some(waiting));
    }
    let target = &command.target;
    let acknowledgement = if let Some(mut recovered) = recovered {
        recovered.replayed = true;
        recovered
    } else {
        tokio::select! {
            biased;
            () = &mut interrupt => return Err(RunOutcome::CancelledBeforeSubmission.into()),
            () = std::future::ready(()) => {},
        }
        // Await an in-flight submission so cancellation can use its receipt.
        submit(&connection, &command).await?
    };
    if acknowledgement.replayed {
        renderer.acknowledged(target, &acknowledgement)?;
        return Ok(None);
    }
    progress.acknowledgement = Some(acknowledgement.clone());
    if args.stream_json {
        write_json(&json!({
            "type": "session", "event": "started", "session_id": target.session_id,
            "agent": target.agent, "receipt": acknowledgement.receipt,
            "replayed": acknowledgement.replayed,
        }))?;
    } else if !args.json {
        eprintln!("session {} agent {}", target.session_id, target.agent);
    }
    settle(
        &connection,
        target,
        acknowledgement,
        Some(renderer),
        args.stream_json,
        None,
        interrupt,
    )
    .await
    .map(Some)
}

/// Reply to one exact durable request and observe the continuation.
pub async fn reply(args: ReplyArgs, options: &ConnectionArgs) -> CliResult {
    let connection = options.connect().await?;
    let has_explicit_id = args.command_id.is_some();
    let target = connection.target(args.session_id, args.agent);
    let action = match args.answer {
        Some(text) => Action::ReplyQuestion {
            request_id: args.request_id,
            text,
        },
        None => Action::ReplyPermission {
            request_id: args.request_id,
            allow: args.allow,
        },
    };
    let command = Command {
        target,
        command_id: args.command_id.unwrap_or_else(new_id),
        action,
    };
    if has_explicit_id {
        if let Some(acknowledgement) = connection
            .pending
            .reconcile(&connection.client, &command)
            .await?
        {
            return render_acknowledgement(&connection, &command.target, acknowledgement, true)
                .await;
        }
    }
    let request = if let Action::ReplyPermission { request_id, .. } = &command.action {
        connection
            .client
            .pending_approvals(&command.target)
            .await?
            .into_iter()
            .find(|approval| {
                approval.request.id == *request_id
                    && approval.request.conversation.as_str() == command.target.agent
            })
    } else {
        None
    };
    let interrupt = std::pin::pin!(ctrl_c_listener()?);
    let acknowledgement = submit(&connection, &command).await?;
    if acknowledgement.replayed {
        return render_acknowledgement(&connection, &command.target, acknowledgement, true).await;
    }
    if let Some(approval) = &request {
        let decision = if args.allow { "allow" } else { "deny" };
        write_json(&permission_decision_line(
            &approval.request,
            decision.into(),
            "host".into(),
        ))?;
    }
    let snapshot = settle(
        &connection,
        &command.target,
        acknowledgement,
        None,
        false,
        request.as_ref(),
        interrupt,
    )
    .await?;
    write_json(&snapshot)?;
    check_outcome(snapshot)
}

async fn prepare_run(
    args: &RunArgs,
    connection: &Connection,
    text: String,
    mut config: AgentConfig,
) -> Result<(Command, Option<Acknowledgement>), Box<dyn std::error::Error>> {
    let requested_session = requested_session(args, connection).await?;
    let pending = match &args.command_id {
        Some(id) => super::pending::find(connection, id, requested_session.as_deref())?,
        None => None,
    };
    let is_creation = match &pending {
        Some(command) => matches!(
            command.action,
            Action::Send {
                create: Some(_),
                ..
            }
        ),
        None => requested_session.is_none(),
    };
    let session_id = pending
        .as_ref()
        .map(|pending| pending.target.session_id.clone())
        .or(requested_session)
        .unwrap_or_else(new_id);
    let target = connection.target(session_id, &args.agent);
    let bootstrap_skill = config.bootstrap_skill.take();
    let prompts_dir = resolve_prompts_dir(connection, args.prompts_dir.as_deref())?;
    let command = Command {
        target,
        command_id: args.command_id.clone().unwrap_or_else(new_id),
        action: Action::Send {
            text: text.clone(),
            timing: Timing::Steer,
            values: Vec::new(),
            images: Vec::new(),
            bootstrap_skill: bootstrap_skill.clone(),
            create: is_creation.then(|| Box::new(config.clone())),
            config_patch: (!is_creation).then(|| Box::new(config)),
            prompts_dir,
            create_agent_if_missing: !is_creation,
        },
    };
    if let Some(pending) = &pending {
        if *pending != command {
            return Err(Error::PendingConflict(command.command_id).into());
        }
        let acknowledgement = connection
            .pending
            .reconcile(&connection.client, pending)
            .await?;
        if acknowledgement.is_some() {
            return Ok((command, acknowledgement));
        }
    } else if !is_creation && args.command_id.is_some() {
        if let Some(acknowledgement) =
            replay_run(connection, &command, text, bootstrap_skill.clone()).await?
        {
            return Ok((command, Some(acknowledgement)));
        }
    }
    Ok((command, None))
}

async fn replay_run(
    connection: &Connection,
    command: &Command,
    text: String,
    bootstrap_skill: Option<String>,
) -> Result<Option<Acknowledgement>, Error> {
    match connection.client.recover(command).await {
        Err(error) if error.code() == Some("command_conflict") => {
            let original_creation_input = Command {
                action: Action::Send {
                    text,
                    timing: Timing::Steer,
                    values: Vec::new(),
                    images: Vec::new(),
                    bootstrap_skill,
                    create: None,
                    config_patch: None,
                    prompts_dir: None,
                    create_agent_if_missing: false,
                },
                ..command.clone()
            };
            connection.client.recover(&original_creation_input).await
        }
        result => result,
    }
}

async fn requested_session(
    args: &RunArgs,
    connection: &Connection,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if !args.cont {
        return Ok(args.session.clone());
    }
    let session = connection
        .client
        .sessions(Some(&connection.workspace), None)
        .await?
        .into_iter()
        .max_by(|left, right| left.created_at.cmp(&right.created_at))
        .ok_or("no sessions found")?;
    Ok(Some(session.session_id))
}

fn run_input(args: &RunArgs) -> Result<(String, AgentConfig), Box<dyn std::error::Error>> {
    let mut text = match args.prompt_file.as_deref() {
        Some(path) if path == Path::new("-") => {
            let mut text = String::new();
            io::stdin()
                .lock()
                .read_to_string(&mut text)
                .map_err(|error| format!("cannot read prompt from stdin: {error}"))?;
            text
        }
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read prompt file {}: {error}", path.display()))?,
        None => args.prompt.join(" "),
    };
    let bootstrap_skill = if args.no_skills || args.prompt_file.is_some() {
        None
    } else {
        slash_command_candidate(&text).map(|(name, rest)| {
            text = rest;
            name
        })
    };
    let config = AgentConfig {
        preset: args.preset.clone(),
        connection: args.connection.clone(),
        model: args.model.clone(),
        reasoning_effort: args.reasoning_effort.clone(),
        service_tier: args.service_tier.clone(),
        max_output_tokens: args.max_output_tokens,
        no_agents: args.no_agents,
        no_skills: args.no_skills,
        bootstrap_skill,
        ..AgentConfig::default()
    };
    Ok((text, config))
}

fn resolve_prompts_dir(
    connection: &Connection,
    path: Option<&Path>,
) -> Result<Option<std::path::PathBuf>, Error> {
    path.map(|path| connection.service_path(path)).transpose()
}

async fn preflight_send(
    connection: &Connection,
    command: &Command,
) -> Result<Option<AgentSnapshot>, Box<dyn std::error::Error>> {
    let Action::Send {
        create,
        bootstrap_skill,
        ..
    } = &command.action
    else {
        return Ok(None);
    };
    if create.is_none()
        && let Some(waiting) = waiting_agent(connection, &command.target).await?
    {
        return Ok(Some(waiting));
    }
    if let Some(name) = bootstrap_skill {
        let skills = connection
            .client
            .skills(&connection.workspace, Some(name))
            .await?;
        if !skills.iter().any(|skill| skill.name == *name) {
            return Err(format!("unknown skill: {name}").into());
        }
    }
    Ok(None)
}

async fn waiting_agent(
    connection: &Connection,
    target: &Target,
) -> Result<Option<AgentSnapshot>, Error> {
    match connection.client.snapshot(target).await {
        Ok(snapshot) if snapshot.agent.status == AgentStatus::Waiting => Ok(Some(snapshot.agent)),
        Ok(_) => Ok(None),
        Err(error) if error.code() == Some("agent_not_found") => Ok(None),
        Err(error) => Err(error),
    }
}

async fn submit(
    connection: &Connection,
    command: &Command,
) -> Result<Acknowledgement, Box<dyn std::error::Error>> {
    connection
        .pending
        .submit(&connection.client, command)
        .await
        .map_err(|error| {
            if error.definitively_rejected() {
                error.into()
            } else {
                format!(
                    "{error}. Check `kuku pending list` for command {}",
                    command.command_id
                )
                .into()
            }
        })
}

// Register synchronously so an interrupt arriving during command submission
// or the initial service reads remains available to the observer.
fn ctrl_c_listener() -> io::Result<impl Future<Output = ()>> {
    #[cfg(unix)]
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    #[cfg(windows)]
    let mut signal = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        signal.recv().await;
    })
}

async fn settle(
    connection: &Connection,
    target: &Target,
    acknowledgement: Acknowledgement,
    mut renderer: Option<&mut RunRenderer>,
    stream_json: bool,
    replied_to: Option<&PendingApproval>,
    mut interrupt: Pin<&mut impl Future<Output = ()>>,
) -> Result<AgentSnapshot, Box<dyn std::error::Error>> {
    let input_id = renderer
        .as_ref()
        .map(|_| acknowledgement.receipt.command_id.as_str());
    if input_id.is_some() && acknowledgement.status == Some(InputStatus::Withdrawn) {
        return Err(RunOutcome::InputWithdrawn.into());
    }
    let receipt_cursor = acknowledgement.receipt.event_id;
    let mut admission = None;
    let mut observer = connection
        .client
        .subscribe_session(target, receipt_cursor.saturating_sub(1));
    let mut settled = connection.client.snapshot(target).await?;
    let mut approvals = connection.client.pending_approvals(target).await?;
    let mut job = match replied_to.and_then(|approval| approval.job_id.as_deref()) {
        Some(id) => Some(connection.client.job(target, id).await?),
        None => None,
    };
    loop {
        if let Some(request) =
            observed_script_approval(&settled.agent, &approvals, receipt_cursor, replied_to)
        {
            settled.agent.permission = Some(request.clone());
            return Ok(settled.agent);
        }
        let agent_complete = is_settled(&settled, receipt_cursor, &approvals);
        if job.is_none()
            && agent_complete
            && settled.agent.running_jobs != 0
            && let Some(approval) =
                replied_to.filter(|approval| approval.request.id.starts_with("script:"))
        {
            // Promotion may have occurred after the pending approval was looked up.
            job = connection
                .client
                .jobs(target)
                .await?
                .into_iter()
                .find(|job| {
                    job.turn == approval.request.turn
                        && job.tool_call_id == approval.request.tool_call_id
                });
        }
        let complete = job
            .as_ref()
            .map_or(agent_complete, |job| job.status != JobStatus::Running);
        if complete && (!stream_json || observer.cursor() >= settled.cursor) {
            break;
        }
        let event = tokio::select! {
            result = observer.next() => result?,
            () = &mut interrupt => {
                if let Some(job) = &job {
                    connection.client.cancel_job(target, &job.id).await?;
                    return Err("job cancelled after cleanup".into());
                }
                if input_id.is_some() {
                    return interrupt_run(connection, target, &acknowledgement, settled, admission).await;
                }
                if settled.execution_id().is_none() {
                    settled = connection.client.snapshot(target).await?;
                }
                if let Some(execution_id) = settled.execution_id() {
                    if let Err(error) = request_cancel(connection, target, execution_id).await {
                        if !execution_not_active(error.as_ref()) {
                            return Err(error);
                        }
                    }
                    return Ok(connection.client.snapshot(target).await?.agent);
                }
                if is_settled(&settled, receipt_cursor, &approvals) {
                    return Ok(settled.agent);
                }
                return Err("no observed Agent execution to cancel".into());
            }
        };
        match event {
            Observation::Committed {
                id,
                agent,
                entry,
                delivery,
                state_changed,
                operation,
                ..
            } => {
                if agent.as_deref() == Some(target.agent.as_str())
                    && let Some(InputDelivery::Accepted { input_ids }) = delivery.as_deref()
                    && input_id.is_some_and(|id| input_ids.iter().any(|input| input == id))
                {
                    admission = Some(id);
                }
                if let (Some(renderer), Some(entry)) = (&mut renderer, entry) {
                    if entry.agent == target.agent {
                        renderer.committed(&entry)?;
                    }
                }
                if state_changed && id > settled.cursor {
                    settled = connection.client.snapshot(target).await?;
                }
                if operation.as_deref() == Some("job_finished") {
                    refresh_observed_job(connection, target, &mut job).await?;
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
            Observation::Live { agent, event, .. } if agent == target.agent => {
                if let Some(renderer) = renderer.as_mut() {
                    renderer.event(event)?;
                }
            }
            Observation::Lagged { skipped } => {
                if stream_json {
                    write_json(&json!({"type":"lagged","skipped":skipped}))?;
                }
                settled = connection.client.snapshot(target).await?;
                approvals = connection.client.pending_approvals(target).await?;
                refresh_observed_job(connection, target, &mut job).await?;
            }
            Observation::Live { .. }
            | Observation::WorkspacePermission { .. }
            | Observation::Heartbeat => {}
        }
    }
    if let Some(renderer) = renderer.filter(|renderer| renderer.needs_preview()) {
        let drain = async {
            for _ in 0..256 {
                match observer.next().await {
                    Ok(Observation::Live { agent, event, .. }) if agent == target.agent => {
                        renderer.event(event)?;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            CliResult::Ok(())
        };
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(20), drain).await {
            result?;
        }
    }
    Ok(settled.agent)
}

async fn interrupt_run(
    connection: &Connection,
    target: &Target,
    acknowledgement: &Acknowledgement,
    mut snapshot: Snapshot,
    observed_admission: Option<u64>,
) -> Result<AgentSnapshot, Box<dyn std::error::Error>> {
    let receipt = &acknowledgement.receipt;
    if acknowledgement.status != Some(InputStatus::Accepted)
        && !observed_admission.is_some_and(|admission| {
            snapshot
                .execution_id()
                .is_some_and(|execution| execution <= admission)
        })
    {
        let withdrawal = submit(
            connection,
            &Command {
                target: target.clone(),
                command_id: new_id(),
                action: Action::Withdraw {
                    input_command_id: receipt.command_id.clone(),
                    draft: None,
                },
            },
        )
        .await?;
        match withdrawal.status {
            Some(InputStatus::Withdrawn) => return Err(RunOutcome::InputWithdrawn.into()),
            Some(InputStatus::Accepted) => {}
            _ => {
                return Err("service did not report the disposition of the submitted input".into());
            }
        }
        snapshot = connection.client.snapshot(target).await?;
    }
    if snapshot.execution_id().is_none() {
        snapshot = connection.client.snapshot(target).await?;
    }
    let Some(execution) = snapshot.execution_id() else {
        return Ok(snapshot.agent);
    };
    let accepted = observed_admission
        .is_some_and(|admission| execution <= admission && admission <= snapshot.cursor);
    if !accepted
        && !input_admitted_during(connection, target, receipt, execution, snapshot.cursor).await?
    {
        return Err("the submitted input is no longer part of the active Agent execution".into());
    }

    match request_cancel(connection, target, execution).await {
        Ok(()) => {
            if let Ok(stopped) = connection.client.snapshot(target).await
                && stopped.agent.status == AgentStatus::Stopped
                && stopped.agent.turn == snapshot.agent.turn
                && !stopped.cleaning_up
            {
                return Ok(stopped.agent);
            }
            Err(RunOutcome::Cancelled.into())
        }
        Err(error) if execution_not_active(error.as_ref()) => {
            let latest = connection.client.snapshot(target).await?;
            if !latest.cleaning_up
                && latest.agent.turn == snapshot.agent.turn
                && latest.execution_id().is_none()
            {
                Ok(latest.agent)
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

async fn input_admitted_during(
    connection: &Connection,
    target: &Target,
    receipt: &Receipt,
    execution: u64,
    snapshot_cursor: u64,
) -> Result<bool, Error> {
    // The execution's ID is the cursor at its start. Admissions are committed
    // after delivery and within that execution's lifetime.
    let mut after = receipt.event_id.max(execution.saturating_sub(1));
    while after < snapshot_cursor {
        let page = connection
            .client
            .events(
                &target.workspace,
                &target.session_id,
                Some(&target.agent),
                after,
                128,
            )
            .await?;
        if page
            .events
            .iter()
            .take_while(|event| event.id <= snapshot_cursor)
            .any(|event| {
                let EventPayload::Unknown(payload) = &event.payload else {
                    return false;
                };
                let record = &payload["record"];
                payload["kind"] == "runtime.operation"
                    && record["operation"] == "inputs_accepted"
                    && record["address"] == target.agent
                    && record["input_ids"].as_array().is_some_and(|inputs| {
                        inputs.iter().any(|input| input == &receipt.command_id)
                    })
            })
        {
            return Ok(true);
        }
        if page.next_after <= after {
            break;
        }
        after = page.next_after;
    }
    Ok(false)
}

async fn request_cancel(connection: &Connection, target: &Target, execution_id: u64) -> CliResult {
    submit(
        connection,
        &Command {
            target: target.clone(),
            command_id: new_id(),
            action: Action::Cancel { execution_id },
        },
    )
    .await?;
    Ok(())
}

fn execution_not_active(error: &(dyn std::error::Error + 'static)) -> bool {
    error.downcast_ref::<Error>().and_then(Error::code) == Some("execution_not_active")
}

async fn refresh_observed_job(
    connection: &Connection,
    target: &Target,
    job: &mut Option<JobSnapshot>,
) -> Result<(), Error> {
    if let Some(job) = job {
        *job = connection.client.job(target, &job.id).await?;
    }
    Ok(())
}

fn observed_script_approval<'a>(
    agent: &AgentSnapshot,
    approvals: &'a [PendingApproval],
    receipt_cursor: u64,
    replied_to: Option<&PendingApproval>,
) -> Option<&'a PermissionRequest> {
    approvals.iter().find_map(|approval| {
        let request = &approval.request;
        if request.conversation.as_str() != agent.address || !request.id.starts_with("script:") {
            return None;
        }
        let current_turn = Some(request.turn) == agent.turn
            && (request.turn >= receipt_cursor
                || approval.job_id.is_none() && agent.status == AgentStatus::Running);
        let same_script = replied_to.is_some_and(|replied| {
            replied.request.turn == request.turn
                && replied.request.tool_call_id == request.tool_call_id
        });
        let relevant = same_script
            || (current_turn && replied_to.is_none_or(|approval| approval.job_id.is_none()));
        relevant.then_some(request)
    })
}

fn is_settled(snapshot: &Snapshot, receipt_cursor: u64, approvals: &[PendingApproval]) -> bool {
    let target_completed = match snapshot.agent.status {
        AgentStatus::Idle => snapshot.agent.pending_inputs == 0,
        AgentStatus::Waiting | AgentStatus::Stopped | AgentStatus::Interrupted => true,
        AgentStatus::Running => false,
    };
    snapshot.cursor >= receipt_cursor
        && target_completed
        && !snapshot.cleaning_up
        && !snapshot.agents.iter().any(|agent| {
            agent.status == AgentStatus::Running
                && !approvals.iter().any(|approval| {
                    let request = &approval.request;
                    approval.job_id.is_none()
                        && request.conversation.as_str() == agent.address
                        && Some(request.turn) == agent.turn
                        && request.id.starts_with("script:")
                })
        })
}

fn check_outcome(snapshot: AgentSnapshot) -> CliResult {
    if let Some(error) = snapshot.last_error {
        return Err(error.into());
    }
    if snapshot.status == AgentStatus::Stopped {
        return Err("agent cancelled after cleanup".into());
    }
    Ok(())
}

async fn render_acknowledgement(
    connection: &Connection,
    target: &Target,
    acknowledgement: Acknowledgement,
    json_output: bool,
) -> CliResult {
    let snapshot = connection.client.snapshot(target).await?.agent;
    let receipt = acknowledgement.receipt;
    if json_output {
        write_json(
            &json!({"type":"session","event":"acknowledged","session_id":target.session_id,
            "agent":target.agent,"replayed":true,"receipt":receipt,"state":snapshot}),
        )?;
    } else {
        eprintln!(
            "command {} already committed at event {}. Session {} agent {}: {:?}",
            receipt.command_id, receipt.event_id, target.session_id, target.agent, snapshot.status
        );
        if let Some(output) = snapshot.last_output {
            println!("{output}");
        }
    }
    Ok(())
}

fn permission_ask_line(request: &PermissionRequest) -> OutputLine {
    OutputLine::permission_ask(
        request.id.clone(),
        request.tool.clone(),
        request.risk.clone(),
        request.summary.clone(),
    )
    .with_conversation(Some(request.conversation.as_str().into()))
}

fn permission_decision_line(
    request: &PermissionRequest,
    decision: String,
    rule: String,
) -> OutputLine {
    OutputLine::permission_decision(request.id.clone(), request.tool.clone(), decision, rule)
        .with_conversation(Some(request.conversation.as_str().into()))
}

fn parse_slash_command(input: &str) -> (String, String) {
    let without_slash = input[1..].trim_start();
    match without_slash.find(char::is_whitespace) {
        Some(pos) => (
            without_slash[..pos].into(),
            without_slash[pos..].trim().into(),
        ),
        None => (without_slash.into(), String::new()),
    }
}

fn slash_command_candidate(input: &str) -> Option<(String, String)> {
    if !input.starts_with('/') {
        return None;
    }
    let (name, rest) = parse_slash_command(input);
    (!name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'))
    .then_some((name, rest))
}

#[cfg(test)]
#[path = "run/tests.rs"]
mod tests;
