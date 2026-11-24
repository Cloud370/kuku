//! Script submission, exact replies, and observation through the Session service.

use std::path::Path;
use std::time::{Duration, Instant};

use kuku_client::{
    Acknowledgement, Action, AgentConfig, AgentSnapshot, AgentStatus, Command, Connection, Error,
    Observation, PendingApproval, PermissionRequest, Snapshot, Target, Timing, new_id,
};
use serde_json::json;

use crate::cli_args::{ConnectionArgs, ReplyArgs, RunArgs};
use crate::commands::CliResult;
use crate::display::OutputLine;

mod render;

use render::{RunRenderer, write_json};

/// Execute one script command, then render the settled Agent state.
pub async fn run(args: RunArgs, options: &ConnectionArgs) -> CliResult {
    let connection = options.connect().await?;
    let (command, recovered) = prepare_run(&args, &connection).await?;
    if let Some(acknowledgement) = recovered {
        return render_acknowledgement(
            &connection,
            &command.target,
            acknowledgement,
            args.json || args.stream_json,
        )
        .await;
    }
    let start = Instant::now();
    let acknowledgement = submit(&connection, &command).await?;
    if acknowledgement.replayed {
        return render_acknowledgement(
            &connection,
            &command.target,
            acknowledgement,
            args.json || args.stream_json,
        )
        .await;
    }
    let target = &command.target;
    if args.stream_json {
        write_json(
            &json!({"type":"session","event":"started","session_id":target.session_id,
            "agent":target.agent,"receipt":acknowledgement.receipt}),
        )?;
    } else if !args.json {
        eprintln!("session {} agent {}", target.session_id, target.agent);
    }
    let mut renderer = RunRenderer::new(&args);
    let snapshot = settle(
        &connection,
        target,
        acknowledgement,
        Some(&mut renderer),
        args.stream_json,
        None,
    )
    .await?;
    renderer.finish(&target.session_id, &snapshot, start.elapsed())?;
    check_outcome(snapshot)
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
            .map(|approval| approval.request)
            .find(|request| {
                request.id == *request_id && request.conversation.as_str() == command.target.agent
            })
    } else {
        None
    };
    let acknowledgement = submit(&connection, &command).await?;
    if acknowledgement.replayed {
        return render_acknowledgement(&connection, &command.target, acknowledgement, true).await;
    }
    if let Some(request) = &request {
        let decision = if args.allow { "allow" } else { "deny" };
        write_json(&permission_decision_line(
            request,
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
    )
    .await?;
    write_json(&snapshot)?;
    check_outcome(snapshot)
}

async fn prepare_run(
    args: &RunArgs,
    connection: &Connection,
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
    let (text, mut config) = run_input(args);
    let bootstrap_skill = config.bootstrap_skill.take();
    let prompts_dir = resolve_prompts_dir(connection, args.prompts_dir.as_deref())?;
    let command = Command {
        target,
        command_id: args.command_id.clone().unwrap_or_else(new_id),
        action: Action::Send {
            text: text.clone(),
            timing: Timing::Steer,
            values: Vec::new(),
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
    if !is_creation {
        check_waiting(connection, &command.target).await?;
    }
    if let Some(name) = &bootstrap_skill {
        let skills = connection
            .client
            .skills(&connection.workspace, Some(name))
            .await?;
        if !skills.iter().any(|skill| skill.name == *name) {
            return Err(format!("unknown skill: {name}").into());
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

fn run_input(args: &RunArgs) -> (String, AgentConfig) {
    let mut text = args.prompt.join(" ");
    let bootstrap_skill = if args.no_skills {
        None
    } else {
        slash_command_candidate(&text).map(|(name, rest)| {
            text = rest;
            name
        })
    };
    let config = AgentConfig {
        connection: args.connection.clone(),
        tier: args.tier.clone(),
        model: args.model.clone(),
        reasoning_effort: args.reasoning_effort.clone(),
        service_tier: args.service_tier.clone(),
        max_output_tokens: args.max_output_tokens,
        no_agents: args.no_agents,
        no_skills: args.no_skills,
        bootstrap_skill,
        ..AgentConfig::default()
    };
    (text, config)
}

fn resolve_prompts_dir(
    connection: &Connection,
    path: Option<&Path>,
) -> Result<Option<std::path::PathBuf>, Error> {
    path.map(|path| connection.service_path(path)).transpose()
}

async fn check_waiting(connection: &Connection, target: &Target) -> CliResult {
    match connection.client.snapshot(target).await {
        Ok(snapshot) if snapshot.agent.status == AgentStatus::Waiting => {
            Err("agent is waiting; use `kuku reply` with its exact pending request identity".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.code() == Some("agent_not_found") => Ok(()),
        Err(error) => Err(error.into()),
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

async fn settle(
    connection: &Connection,
    target: &Target,
    acknowledgement: Acknowledgement,
    mut renderer: Option<&mut RunRenderer>,
    stream_json: bool,
    replied_to: Option<&PermissionRequest>,
) -> Result<AgentSnapshot, Box<dyn std::error::Error>> {
    let receipt_cursor = acknowledgement.receipt.event_id;
    let mut observer = connection
        .client
        .subscribe_session(target, receipt_cursor.saturating_sub(1));
    let mut settled = connection.client.snapshot(target).await?;
    let mut approvals = connection.client.pending_approvals(target).await?;
    loop {
        if let Some(request) =
            observed_script_approval(&settled.agent, &approvals, receipt_cursor, replied_to)
        {
            settled.agent.permission = Some(request.clone());
            return Ok(settled.agent);
        }
        if is_settled(&settled, receipt_cursor, &approvals)
            && (!stream_json || observer.cursor() >= settled.cursor)
        {
            break;
        }
        let event = tokio::select! {
            result = observer.next() => result?,
            signal = tokio::signal::ctrl_c() => {
                signal?;
                if let Some(execution_id) = settled.execution_id() {
                    submit(connection, &Command { target: target.clone(), command_id: new_id(), action: Action::Cancel { execution_id } }).await?;
                    return Ok(connection.client.snapshot(target).await?.agent);
                }
                return Err("no observed Agent execution to cancel".into());
            }
        };
        match event {
            Observation::Committed {
                id,
                entry,
                state_changed,
                operation,
                ..
            } => {
                if let (Some(renderer), Some(entry)) = (&mut renderer, entry) {
                    if entry.agent == target.agent {
                        renderer.committed(&entry)?;
                    }
                }
                if state_changed && id > settled.cursor {
                    settled = connection.client.snapshot(target).await?;
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
            }
            Observation::Live { .. } | Observation::Heartbeat => {}
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

fn observed_script_approval<'a>(
    agent: &AgentSnapshot,
    approvals: &'a [PendingApproval],
    receipt_cursor: u64,
    replied_to: Option<&PermissionRequest>,
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
            replied.turn == request.turn && replied.tool_call_id == request.tool_call_id
        });
        (current_turn || same_script).then_some(request)
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
