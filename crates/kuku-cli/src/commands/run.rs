//! Script submission, exact replies, and observation through the Session service.

use std::fmt;
use std::future::Future;
use std::io::{self, Read};
use std::path::Path;
use std::time::Instant;

use kuku_client::{
    Acknowledgement, Action, AgentConfig, AgentSnapshot, AgentStatus, Command, Connection, Error,
    HistoryKind, PendingApproval, PermissionRequest, Snapshot, Target, Timing, new_id,
};
use serde_json::json;

use crate::cli_args::{ConnectionArgs, ReplyArgs, RunArgs};
use crate::commands::CliResult;
use crate::display::OutputLine;

mod observe;
mod render;

use observe::{RunResult, observe_command};
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
    let result = match execute_run(&args, options, &mut renderer, &mut progress).await {
        Ok(Some(result)) => result,
        Ok(None) => return Ok(()),
        Err(error) => {
            renderer.finish_error(&progress, &args.agent, error.as_ref(), start.elapsed())?;
            return Err(error);
        }
    };
    let outcome = result.outcome();
    renderer.finish(&progress, &args.agent, &result, outcome, start.elapsed())?;
    match outcome {
        RunOutcome::Completed => Ok(()),
        RunOutcome::Failed => Err(result.error.unwrap_or_else(|| outcome.to_string()).into()),
        outcome => Err(outcome.into()),
    }
}

async fn execute_run(
    args: &RunArgs,
    options: &ConnectionArgs,
    renderer: &mut RunRenderer,
    progress: &mut RunProgress,
) -> Result<Option<RunResult>, Box<dyn std::error::Error>> {
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
        return Ok(Some(RunResult::waiting(&waiting)));
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
    observe_command(
        &connection,
        &command,
        &acknowledgement,
        None,
        Some(renderer),
        interrupt,
    )
    .await
    .map(|(result, _)| Some(result))
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
    let request = match (&command.action, request) {
        // A request can arrive between listing approvals and submitting the exact reply.
        (Action::ReplyPermission { request_id, .. }, None) => Some(
            committed_approval(
                &connection,
                &command.target,
                request_id,
                acknowledgement.receipt.event_id,
            )
            .await?,
        ),
        (_, request) => request,
    };
    if let Some(approval) = &request {
        let decision = if args.allow { "allow" } else { "deny" };
        write_json(&permission_decision_line(
            &approval.request,
            decision.into(),
            "host".into(),
        ))?;
    }
    let (result, snapshot) = observe_command(
        &connection,
        &command,
        &acknowledgement,
        request.as_ref(),
        None,
        interrupt,
    )
    .await?;
    result.finish_reply(&snapshot)
}

async fn committed_approval(
    connection: &Connection,
    target: &Target,
    request_id: &str,
    mut before: u64,
) -> Result<PendingApproval, Box<dyn std::error::Error>> {
    loop {
        let page = connection.client.history(target, Some(before), 64).await?;
        if let Some(entry) = page.entries.into_iter().find(|entry| {
            entry.kind == HistoryKind::Permission && entry.details["request"]["id"] == request_id
        }) {
            return Ok(PendingApproval {
                request: serde_json::from_value(entry.details["request"].clone())?,
                job_id: None,
            });
        }
        before = page
            .next_before
            .filter(|cursor| *cursor < before)
            .ok_or("the committed permission request is missing from the Agent history")?;
    }
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

fn running_peers(snapshot: &Snapshot, approvals: &[PendingApproval]) -> bool {
    snapshot.agents.iter().any(|agent| {
        agent.address != snapshot.agent.address
            && agent.status == AgentStatus::Running
            && !approvals.iter().any(|approval| {
                let request = &approval.request;
                approval.job_id.is_none()
                    && request.conversation.as_str() == agent.address
                    && Some(request.turn) == agent.turn
                    && request.id.starts_with("script:")
            })
    })
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
