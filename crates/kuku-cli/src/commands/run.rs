//! Local input, rendering, and cleanup over the public Session SDK.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

use kuku::session::{AgentConfig, AgentStatus, Input, Observation, Session, SessionOptions};
use kuku::{ToolEvent, UiEvent};
use serde_json::json;

use crate::cli_args::{ReplyArgs, RunArgs};
use crate::display::OutputLine;

mod render;

use render::RunRenderer;

type HostResult<T> = Result<T, Box<dyn std::error::Error>>;

struct PreparedRun {
    options: SessionOptions,
    agent: AgentConfig,
    prompt: String,
    update_config: bool,
}

/// Execute a task through the same persistent Session API used by embedded callers.
pub async fn run(args: RunArgs) -> HostResult<()> {
    let prepared = prepare_run(&args, resolve_config_path(args.config.as_deref())?)?;
    let session = Session::open(prepared.options).await?;
    let result = run_open_session(
        &args,
        &session,
        prepared.agent,
        prepared.prompt,
        prepared.update_config,
    )
    .await;
    let close = session.close().await;
    finish_close(result, close)
}

/// Reply to a matching durable request, observe its continuation, and await cleanup.
pub async fn reply(args: ReplyArgs) -> HostResult<()> {
    let config = load_config(resolve_config_path(args.config.as_deref())?)?;
    let workspace = workspace_path(args.workspace.as_deref())?;
    let home = data_path(args.data_dir.as_deref())?;
    if !kuku::session::session_events_path(&home, &workspace, &args.session_id)?.exists() {
        return Err(format!(
            "session '{}' does not exist in this workspace",
            args.session_id
        )
        .into());
    }
    let mut options = SessionOptions::new(workspace, home, config);
    options.session_id = Some(args.session_id);
    let session = Session::open(options).await?;
    let command_id = args.command_id.unwrap_or_else(command_id);
    let result = async {
        let pending = session.agent(&args.agent)?.permission;
        let receipt = match args.answer.as_ref() {
            Some(text) => session.reply_question(&command_id, &args.agent, &args.request_id, text)?,
            None => session.reply_permission(&command_id, &args.agent, &args.request_id, args.allow)?,
        };
        if let Some(request) = pending {
            println!("{}", permission_decision_line(&request, if args.allow { "allow" } else { "deny" }.into(), "host".into()).to_json_line());
        }
        let mut observer = session.observe(receipt.event_id);
        loop {
            let snapshot = session.agent(&args.agent)?;
            if snapshot.status != AgentStatus::Running && !session.agents().iter().any(|agent| agent.status == AgentStatus::Running) {
                println!("{}", serde_json::to_string(&snapshot)?);
                if let Some(error) = snapshot.last_error { return Err(error.into()) }
                if snapshot.status == AgentStatus::Stopped { return Err("agent cancelled after cleanup".into()) }
                return Ok::<(), Box<dyn std::error::Error>>(());
            }
            tokio::select! {
                event = observer.next() => { if event?.is_none() { return Err("session observation ended before continuation settled".into()); } }
                _ = tokio::signal::ctrl_c() => {
                    session.cancel(&args.agent).await?;
                    return Err("agent cancelled after cleanup".into());
                }
            }
        }
    }.await;
    let close = session.close().await;
    finish_close(result, close)
}

/// Read local prompts until EOF, preserving the same Session between inputs.
pub async fn interactive(config: Option<String>) -> HostResult<()> {
    let mut session_id = None;
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let prompt = input.trim();
        if matches!(prompt, "exit" | "quit") {
            break;
        }
        if prompt.is_empty() {
            continue;
        }
        let args = RunArgs {
            prompt: vec![prompt.into()],
            auto_yes: true,
            model: None,
            connection: None,
            command_id: None,
            tier: None,
            agent: "main".into(),
            workspace: None,
            data_dir: None,
            reasoning_effort: None,
            service_tier: None,
            max_output_tokens: None,
            session: session_id.clone(),
            cont: false,
            json: false,
            stream_json: false,
            show_thinking: false,
            raw: false,
            verbose: false,
            config: config.clone(),
            prompts_dir: None,
            no_agents: false,
            no_skills: false,
            skill_body: None,
            bootstrap_skill_name: None,
        };
        let prepared = prepare_run(&args, resolve_config_path(args.config.as_deref())?)?;
        let session = Session::open(prepared.options).await?;
        session_id = Some(session.id().to_string());
        let result = run_open_session(
            &args,
            &session,
            prepared.agent,
            prepared.prompt,
            prepared.update_config,
        )
        .await;
        let close = session.close().await;
        if let Err(error) = result {
            eprintln!("error: {error}");
        }
        close?;
    }
    Ok(())
}

async fn run_open_session(
    args: &RunArgs,
    session: &Session,
    config: AgentConfig,
    prompt: String,
    update_config: bool,
) -> HostResult<()> {
    let prior = session
        .agents()
        .into_iter()
        .find(|agent| agent.address == args.agent);
    if prior
        .as_ref()
        .is_some_and(|agent| agent.status == AgentStatus::Waiting)
    {
        return Err(
            "agent is waiting; use `kuku reply` with its exact pending request identity".into(),
        );
    }
    match prior {
        None => {
            session.create_agent(&command_id(), &args.agent, config)?;
        }
        Some(snapshot) if update_config || snapshot.config.bootstrap_skill.is_some() => {
            session.configure_agent(
                &command_id(),
                &args.agent,
                merge_agent_config(snapshot.config, config),
            )?;
        }
        Some(_) => {}
    }
    let mut renderer = RunRenderer::new(args);
    let start = Instant::now();
    let receipt = session.send(Input::new(
        args.command_id.clone().unwrap_or_else(command_id),
        &args.agent,
        prompt,
    ))?;
    let mut observer = session.observe(receipt.event_id.saturating_sub(1));
    if matches!(
        session.agent(&args.agent)?.status,
        AgentStatus::Stopped | AgentStatus::Interrupted
    ) {
        session.advance(&args.agent)?;
    }
    if args.stream_json {
        println!(
            "{}",
            json!({"type":"session", "event":"started", "session_id": session.id(), "agent":args.agent,"receipt":receipt})
        );
    } else if !args.json {
        eprintln!("session {} agent {}", session.id(), args.agent);
    }
    loop {
        let event = tokio::select! {
            event = observer.next() => event?,
            _ = tokio::signal::ctrl_c() => {
                session.cancel(&args.agent).await?;
                break;
            }
        };
        match event {
            Some(Observation::Live { agent, event }) if agent == args.agent => {
                renderer.event(*event)
            }
            Some(Observation::Live { .. }) => {}
            Some(Observation::Committed(event)) if event.id >= receipt.event_id => {
                let snapshot = session.agent(&args.agent)?;
                let target_settled = snapshot.status == AgentStatus::Waiting
                    || (snapshot.status != AgentStatus::Running && snapshot.pending_inputs == 0);
                if target_settled
                    && !session
                        .agents()
                        .iter()
                        .any(|agent| agent.status == AgentStatus::Running)
                {
                    break;
                }
            }
            Some(Observation::Committed(_)) => {}
            Some(Observation::Lagged { skipped }) => {
                if args.stream_json {
                    println!("{}", json!({"type":"lagged","skipped":skipped}));
                }
            }
            None => return Err("session observation ended before the agent settled".into()),
        }
    }
    for _ in 0..256 {
        match tokio::time::timeout(std::time::Duration::from_millis(1), observer.next()).await {
            Ok(Ok(Some(Observation::Live { agent, event }))) if agent == args.agent => {
                renderer.event(*event)
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Err(_) => break,
            Ok(Err(error)) => return Err(error.into()),
        }
    }
    let snapshot = session.agent(&args.agent)?;
    renderer.finish(session, &snapshot, start.elapsed());
    if let Some(error) = snapshot.last_error {
        return Err(error.into());
    }
    if snapshot.status == AgentStatus::Stopped {
        return Err("agent cancelled after cleanup".into());
    }
    Ok(())
}

fn prepare_run(args: &RunArgs, config_path: PathBuf) -> HostResult<PreparedRun> {
    let config = load_config(config_path)?;
    let workspace = workspace_path(args.workspace.as_deref())?;
    let home = data_path(args.data_dir.as_deref())?;
    let mut prompt = args.prompt.join(" ");
    let mut bootstrap_skill = args.bootstrap_skill_name.clone();
    if !args.no_skills {
        if let Some((name, rest)) = slash_command_candidate(&prompt) {
            let registry =
                kuku::skill::build_registry_snapshot_for_host(&home, &workspace, &config)?;
            if registry.get(&name).is_none() {
                return Err(format!("unknown skill: {name}").into());
            }
            bootstrap_skill = Some(name);
            prompt = rest;
        }
    }
    if args.skill_body.is_some() && bootstrap_skill.is_none() {
        return Err("bootstrap skill input requires its skill name".into());
    }
    let session_id = if args.cont {
        Some(
            kuku::session::list_sessions(&home, Some(&workspace))?
                .into_iter()
                .max_by(|left, right| left.created_at.cmp(&right.created_at))
                .ok_or("no sessions found")?
                .session_id,
        )
    } else {
        args.session.clone()
    };
    if let Some(id) = &session_id {
        if !kuku::session::session_events_path(&home, &workspace, id)?.exists() {
            return Err(format!("session '{id}' does not exist in this workspace").into());
        }
    }
    let mut options = SessionOptions::new(workspace, home, config);
    options.session_id = session_id;
    options.prompts_dir = args.prompts_dir.as_ref().map(PathBuf::from);
    let agent = AgentConfig {
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
    let update_config = agent != AgentConfig::default();
    Ok(PreparedRun {
        options,
        agent,
        prompt,
        update_config,
    })
}

fn merge_agent_config(mut current: AgentConfig, update: AgentConfig) -> AgentConfig {
    if update.connection.is_some() || update.model.is_some() || update.tier.is_some() {
        current.reasoning_effort = None;
        current.service_tier = None;
    }
    if update.connection.is_some() {
        current.connection = update.connection;
    }
    if update.model.is_some() {
        current.model = update.model;
    }
    if update.tier.is_some() {
        current.tier = update.tier;
        current.model = None;
    }
    if update.reasoning_effort.is_some() {
        current.reasoning_effort = update.reasoning_effort;
    }
    if update.service_tier.is_some() {
        current.service_tier = update.service_tier;
    }
    if update.max_output_tokens.is_some() {
        current.max_output_tokens = update.max_output_tokens;
    }
    current.no_agents |= update.no_agents;
    current.no_skills |= update.no_skills;
    current.bootstrap_skill = update.bootstrap_skill;
    current
}

fn resolve_config_path(custom: Option<&str>) -> HostResult<PathBuf> {
    Ok(match custom {
        Some(path) => PathBuf::from(path),
        None => data_path(None)?.join("config.toml"),
    })
}

fn load_config(path: PathBuf) -> HostResult<kuku::config::Config> {
    if !path.exists() {
        return Err(format!("config file not found: {}; run `kuku init`", path.display()).into());
    }
    Ok(kuku::config::load_config(&path)?.resolve()?)
}

fn workspace_path(path: Option<&str>) -> HostResult<PathBuf> {
    Ok(match path {
        Some(path) => std::fs::canonicalize(path)?,
        None => kuku::session::current_workspace()?,
    })
}

fn data_path(path: Option<&str>) -> HostResult<PathBuf> {
    Ok(match path {
        Some(path) => PathBuf::from(path),
        None => kuku::session::kuku_home()?,
    })
}

fn command_id() -> String {
    kuku::session::new_session_id()
}

fn finish_close(result: HostResult<()>, close: kuku::Result<()>) -> HostResult<()> {
    match (result, close) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(error), Err(cleanup)) => {
            Err(format!("{error}; session cleanup failed: {cleanup}").into())
        }
    }
}

fn nested_permission_parent_tool_id(event: &UiEvent) -> Option<&str> {
    match event {
        UiEvent::ToolOutput {
            id,
            event: ToolEvent::PermissionRequested { .. },
        } => Some(id.as_str()),
        _ => None,
    }
}

fn nested_model_recovery_info(event: &UiEvent) -> Option<&kuku::query::ModelRecoveryInfo> {
    match event {
        UiEvent::ToolOutput {
            event: ToolEvent::ModelRecovery { info },
            ..
        } => Some(info),
        _ => None,
    }
}

fn conversation_for_tool_kind(kind: &kuku::query::ToolKind) -> Option<String> {
    match kind {
        kuku::query::ToolKind::Agent { conversation, .. } => Some(conversation.as_str().into()),
        _ => None,
    }
}

fn tool_call_line(
    tool: String,
    id: String,
    summary: String,
    kind: &kuku::query::ToolKind,
) -> OutputLine {
    OutputLine::tool_call(tool, id, summary, serde_json::Value::Null)
        .with_conversation(conversation_for_tool_kind(kind))
}

fn tool_result_line(
    id: String,
    status: String,
    summary: String,
    conversation: Option<String>,
) -> OutputLine {
    OutputLine::tool_result(id, status, summary, None, false).with_conversation(conversation)
}

fn permission_ask_line(request: &kuku::query::PermissionRequest) -> OutputLine {
    OutputLine::permission_ask(
        request.id.clone(),
        request.tool.clone(),
        request.risk.clone(),
        request.summary.clone(),
    )
    .with_conversation(Some(request.conversation.as_str().into()))
}

fn permission_decision_line(
    request: &kuku::query::PermissionRequest,
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
            without_slash[..pos].to_string(),
            without_slash[pos..].trim().to_string(),
        ),
        None => (without_slash.to_string(), String::new()),
    }
}

fn slash_command_candidate(input: &str) -> Option<(String, String)> {
    if !input.starts_with('/') {
        return None;
    }
    let (name, rest) = parse_slash_command(input);
    if name.is_empty()
        || name.contains('/')
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return None;
    }
    Some((name, rest))
}

#[cfg(test)]
#[path = "run/tests.rs"]
mod tests;
