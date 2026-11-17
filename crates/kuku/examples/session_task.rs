use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kuku::event::EventPayload;
use kuku::{AgentConfig, AgentStatus, Input, Session, SessionOptions};
use serde_json::json;

type AnyError = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() {
    match execute().await {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

async fn execute() -> Result<bool, AnyError> {
    let args = arguments()?;
    if args.contains_key("help") {
        println!("session_task --workspace DIR --prompt-file FILE --config FILE --data-dir DIR --output FILE [--session-id ID --agent main --connection NAME --model MODEL --tier TIER --reasoning-effort low --service-tier VALUE --max-steps 12 --max-output-tokens 2048 --max-context-tokens 65536 --timeout 300 --command-id KEY]");
        return Ok(true);
    }
    let workspace = PathBuf::from(required(&args, "workspace")?);
    let data_dir = PathBuf::from(required(&args, "data-dir")?);
    let output_path = PathBuf::from(required(&args, "output")?);
    let prompt = std::fs::read_to_string(required(&args, "prompt-file")?)?;
    let mut config =
        kuku::config::load_config(&PathBuf::from(required(&args, "config")?))?.resolve()?;
    let max_steps = number(&args, "max-steps", 12_u64)?;
    let max_output_tokens = number(&args, "max-output-tokens", 2048_u32)?;
    let max_context_tokens = number(&args, "max-context-tokens", 65_536_u32)?;
    let timeout = number(&args, "timeout", 300_u64)?;
    for tier in config.tiers.values_mut() {
        tier.context_window = tier.context_window.min(max_context_tokens);
    }
    let mut options = SessionOptions::new(workspace, data_dir, config);
    options.session_id = args.get("session-id").cloned();
    options.prompts_dir = args.get("prompts-dir").map(PathBuf::from);
    options.limits.max_steps_per_turn = max_steps;
    options.limits.max_request_bytes = usize::try_from(max_context_tokens)?;
    let limits = options.limits.clone();
    let session = Session::open(options).await?;
    let agent = args.get("agent").map(String::as_str).unwrap_or("main");
    let selection = AgentConfig {
        connection: args.get("connection").cloned(),
        tier: args.get("tier").cloned(),
        model: args.get("model").cloned(),
        reasoning_effort: args.get("reasoning-effort").cloned(),
        service_tier: args.get("service-tier").cloned(),
        max_output_tokens: Some(max_output_tokens),
        no_agents: args.contains_key("no-agents"),
        no_skills: args.contains_key("no-skills"),
        ..Default::default()
    };
    let command_id = args
        .get("command-id")
        .cloned()
        .unwrap_or_else(kuku::session::new_session_id);
    let started = Instant::now();
    let task = async {
        match session.agent(agent) {
            Ok(existing) if existing.config != selection => {
                session.configure_agent(&format!("configure:{command_id}"), agent, selection)?;
            }
            Ok(_) => {}
            Err(_) => {
                session.create_agent(&format!("create:{agent}"), agent, selection)?;
            }
        }
        let receipt = session.send(Input::new(command_id, agent, prompt))?;
        session.advance(agent)?;
        let mut observer = session.observe(receipt.event_id);
        loop {
            let snapshot = session.agent(agent)?;
            match snapshot.status {
                AgentStatus::Idle if snapshot.pending_inputs == 0 => {
                    return Ok::<_, kuku::Error>(snapshot)
                }
                AgentStatus::Waiting | AgentStatus::Stopped | AgentStatus::Interrupted => {
                    return Ok(snapshot)
                }
                AgentStatus::Idle | AgentStatus::Running => {}
            }
            if observer.next().await?.is_none() {
                return Err(kuku::Error::InvalidEventStream(
                    "session closed before execution settled".into(),
                ));
            }
        }
    };
    let mut error = None;
    let snapshot = match tokio::time::timeout(Duration::from_secs(timeout), task).await {
        Ok(Ok(snapshot)) => Some(snapshot),
        Ok(Err(failure)) => {
            error = Some(failure.to_string());
            session.agent(agent).ok()
        }
        Err(_) => {
            error = Some(format!("task exceeded {timeout}s"));
            if let Err(failure) = session.cancel(agent).await {
                error = Some(failure.to_string());
            }
            session.agent(agent).ok()
        }
    };
    let close = session.close().await;
    if let Err(failure) = &close {
        error = Some(failure.to_string());
    }
    let mut after = 0;
    let mut facts = BTreeMap::<&str, u64>::new();
    let mut settings = Vec::new();
    let mut reports = Vec::new();
    loop {
        let events = session.events(after, 128)?;
        if events.is_empty() {
            break;
        }
        for event in events {
            after = event.id;
            let key = match &event.payload {
                EventPayload::ToolCall { conversation, .. }
                    if conversation.as_deref().unwrap_or("main") == agent =>
                {
                    Some("tool_calls")
                }
                EventPayload::ToolResult { conversation, .. }
                    if conversation.as_deref().unwrap_or("main") == agent =>
                {
                    Some("tool_results")
                }
                EventPayload::ModelResponse { conversation, .. }
                    if conversation.as_deref().unwrap_or("main") == agent =>
                {
                    Some("model_responses")
                }
                EventPayload::TurnCompleted { conversation, .. } if conversation == agent => {
                    Some("turns_completed")
                }
                EventPayload::Unknown(value)
                    if value["agent"] == agent && value["kind"] == "runtime.step_prepared" =>
                {
                    settings.push(value.clone());
                    Some("prepared_steps")
                }
                EventPayload::Unknown(value)
                    if value["agent"] == agent && value["kind"] == "runtime.provider_report" =>
                {
                    reports.push(value.clone());
                    None
                }
                _ => None,
            };
            if let Some(key) = key {
                *facts.entry(key).or_default() += 1;
            }
        }
    }
    let passed = error.is_none()
        && snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.status == AgentStatus::Idle && snapshot.pending_inputs == 0
        });
    let result = json!({"session_id":session.id(),"agent":snapshot,"events_path":session.events_path(),"last_event_id":after,"facts":facts,"provider_settings":settings,"provider_reports":reports,"limits":{"max_steps":limits.max_steps_per_turn,"max_output_tokens":max_output_tokens,"max_request_bytes":limits.max_request_bytes,"max_context_tokens":max_context_tokens,"automatic_retries":0},"elapsed_ms":started.elapsed().as_millis(),"cleanup":{"closed":close.is_ok()},"runtime_completed":passed,"error":error});
    if let Some(parent) = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, serde_json::to_vec_pretty(&result)?)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(passed)
}

fn arguments() -> Result<BTreeMap<String, String>, AnyError> {
    let mut values = std::env::args().skip(1);
    let mut args = BTreeMap::new();
    while let Some(flag) = values.next() {
        let name = flag
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected positional argument '{flag}'"))?;
        if matches!(name, "help" | "no-agents" | "no-skills") {
            args.insert(name.into(), String::new());
            continue;
        }
        if !matches!(
            name,
            "workspace"
                | "prompt-file"
                | "config"
                | "data-dir"
                | "output"
                | "session-id"
                | "agent"
                | "connection"
                | "model"
                | "tier"
                | "reasoning-effort"
                | "service-tier"
                | "max-steps"
                | "max-output-tokens"
                | "max-context-tokens"
                | "timeout"
                | "command-id"
                | "prompts-dir"
        ) {
            return Err(format!("unknown option '--{name}'").into());
        }
        args.insert(
            name.into(),
            values
                .next()
                .ok_or_else(|| format!("missing value for '--{name}'"))?,
        );
    }
    Ok(args)
}

fn required<'a>(args: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, AnyError> {
    args.get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("required option '--{name}' is missing").into())
}

fn number<T: std::str::FromStr>(
    args: &BTreeMap<String, String>,
    name: &str,
    default: T,
) -> Result<T, AnyError> {
    match args.get(name) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("invalid numeric value for '--{name}'").into()),
        None => Ok(default),
    }
}
