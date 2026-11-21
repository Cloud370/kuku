#[path = "support/process_resources.rs"]
mod process_resources;

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kuku::event::EventPayload;
use kuku::execution::CommandExecutionFacts;
use kuku::query::ModelPhase;
use kuku::{
    AgentConfig, AgentStatus, Input, NamedValueSource, Observation, OutputRead, Session,
    SessionOptions, ToolEvent, UiEvent, WorkspaceScriptFacts,
};
use serde_json::{json, Value};

type AnyError = Box<dyn std::error::Error>;

#[derive(Default, serde::Serialize)]
struct StreamMetrics {
    model_text_chunks: u64,
    model_thinking_chunks: u64,
    tool_output_chunks: u64,
    model_text_bytes: usize,
    model_thinking_bytes: usize,
    tool_output_bytes: usize,
    first_model_text_ms: Option<u128>,
    first_model_thinking_ms: Option<u128>,
    first_tool_output_ms: Option<u128>,
    first_model_request_ms: Option<u128>,
    model_request_events: u64,
    model_phase_events: BTreeMap<&'static str, u64>,
    first_model_phase_ms: BTreeMap<&'static str, u128>,
    tool_starts: u64,
    first_tool_start_ms: Option<u128>,
    committed_events: u64,
    lagged_updates: u64,
}

impl StreamMetrics {
    fn observe(&mut self, observation: Observation, agent: &str, started: Instant) {
        let elapsed = started.elapsed().as_millis();
        match observation {
            Observation::Live {
                agent: source,
                event,
                ..
            } if source == agent => match *event {
                UiEvent::TextDelta { text } => {
                    self.model_text_chunks += 1;
                    self.model_text_bytes += text.len();
                    self.first_model_text_ms.get_or_insert(elapsed);
                }
                UiEvent::ThinkingDelta { text } => {
                    self.model_thinking_chunks += 1;
                    self.model_thinking_bytes += text.len();
                    self.first_model_thinking_ms.get_or_insert(elapsed);
                }
                UiEvent::ToolOutput {
                    event: ToolEvent::Stdout { text } | ToolEvent::Stderr { text },
                    ..
                } => {
                    self.tool_output_chunks += 1;
                    self.tool_output_bytes += text.len();
                    self.first_tool_output_ms.get_or_insert(elapsed);
                }
                UiEvent::ModelRequest { .. } => {
                    self.model_request_events += 1;
                    self.first_model_request_ms.get_or_insert(elapsed);
                }
                UiEvent::ModelProgress { phase } => {
                    let name = match phase {
                        ModelPhase::Thinking => "thinking",
                        ModelPhase::Streaming => "streaming",
                        ModelPhase::ToolArguments => "tool_arguments",
                    };
                    *self.model_phase_events.entry(name).or_default() += 1;
                    self.first_model_phase_ms.entry(name).or_insert(elapsed);
                }
                UiEvent::ToolStart { .. } => {
                    self.tool_starts += 1;
                    self.first_tool_start_ms.get_or_insert(elapsed);
                }
                _ => {}
            },
            Observation::Committed(_) => self.committed_events += 1,
            Observation::Lagged { skipped } => self.lagged_updates += skipped,
            Observation::Live { .. } | Observation::ReplyPreview { .. } => {}
        }
    }
}

#[cfg(not(test))]
#[tokio::main]
async fn main() {
    let result = match arguments(std::env::args().skip(1)) {
        Ok(args) => execute(args).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn execute(args: BTreeMap<String, String>) -> Result<bool, AnyError> {
    if args.contains_key("help") {
        println!("session_task --workspace DIR --prompt-file FILE --config FILE --data-dir DIR --output FILE [--session-id ID --agent main --connection NAME --model MODEL --tier TIER --reasoning-effort low --service-tier VALUE --max-requests N --no-provider-retry --max-agents 64 --max-output-tokens N --max-context-tokens 131072 --max-request-bytes BYTES --timeout 300 --command-id KEY --inspect-named-value NAME --seed-named-json FILE]");
        println!("--max-requests optionally caps provider attempts across this open session. --no-provider-retry disables the default pre-stream retry.");
        println!("--max-output-tokens optionally caps each model request. Omit it to use the selected tier or provider default.");
        println!("--max-request-bytes caps the serialized provider request. It defaults to the SDK's request byte limit.");
        println!("--max-command-seconds sets a host deadline for each tool command, including script commands and Jobs.");
        return Ok(true);
    }
    let workspace = PathBuf::from(required(&args, "workspace")?);
    let data_dir = PathBuf::from(required(&args, "data-dir")?);
    let output_path = PathBuf::from(required(&args, "output")?);
    let prompt = std::fs::read_to_string(required(&args, "prompt-file")?)?;
    let mut config =
        kuku::config::load_config(&PathBuf::from(required(&args, "config")?))?.resolve()?;
    let max_requests = optional_number(&args, "max-requests")?;
    let max_agents = number(&args, "max-agents", 64_usize)?;
    let max_output_tokens = optional_number(&args, "max-output-tokens")?;
    let max_context_tokens = number(&args, "max-context-tokens", 131_072_u32)?;
    if max_context_tokens == 0 {
        return Err("--max-context-tokens must be positive".into());
    }
    let timeout = number(&args, "timeout", 300_u64)?;
    for tier in config.tiers.values_mut() {
        tier.context_window = tier.context_window.min(max_context_tokens);
    }
    let mut options = SessionOptions::new(workspace, data_dir, config);
    options.session_id = args.get("session-id").cloned();
    options.prompts_dir = args.get("prompts-dir").map(PathBuf::from);
    if let Some(max_requests) = max_requests {
        options.limits.max_requests_per_open = Some(max_requests);
    }
    if args.contains_key("no-provider-retry") {
        options.limits.retry_transient_provider_errors = false;
    }
    options.limits.max_output_tokens_per_request = max_output_tokens;
    options.limits.max_agents = max_agents;
    options.limits.max_request_bytes =
        number(&args, "max-request-bytes", options.limits.max_request_bytes)?;
    options.limits.max_command_seconds =
        optional_number::<NonZeroU32>(&args, "max-command-seconds")?;
    let limits = options.limits.clone();
    let inspected_name = args.get("inspect-named-value");
    let seed = args
        .get("seed-named-json")
        .map(|path| -> Result<_, AnyError> {
            let name = inspected_name.ok_or("--seed-named-json requires --inspect-named-value")?;
            let json = serde_json::from_slice(&std::fs::read(path)?)?;
            Ok((name, NamedValueSource::Json(json)))
        })
        .transpose()?;
    let session = Session::open(options).await?;
    let agent = args.get("agent").map(String::as_str).unwrap_or("main");
    let mut named_before = None;
    let selection = AgentConfig {
        connection: args.get("connection").cloned(),
        tier: args.get("tier").cloned(),
        model: args.get("model").cloned(),
        reasoning_effort: args.get("reasoning-effort").cloned(),
        service_tier: args.get("service-tier").cloned(),
        max_output_tokens,
        no_agents: args.contains_key("no-agents"),
        no_skills: args.contains_key("no-skills"),
        ..Default::default()
    };
    let command_id = args
        .get("command-id")
        .cloned()
        .unwrap_or_else(kuku::session::new_session_id);
    let started = Instant::now();
    let mut streaming = StreamMetrics::default();
    let mut observer = session.observe(0);
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
        if let Some((name, source)) = seed {
            session.save_named_value(agent, name, source, false).await?;
        }
        named_before = inspected_name.map(|name| inspect_named_value(&session, agent, name));
        session.send(Input::new(command_id, agent, prompt))?;
        session.advance(agent)?;
        loop {
            let snapshot = session.agent(agent)?;
            let others_running = session
                .agents()
                .iter()
                .any(|state| state.status == AgentStatus::Running && state.address != agent);
            match snapshot.status {
                AgentStatus::Idle if !others_running => return Ok::<_, kuku::Error>(snapshot),
                AgentStatus::Waiting | AgentStatus::Stopped | AgentStatus::Interrupted
                    if !others_running =>
                {
                    return Ok(snapshot)
                }
                AgentStatus::Idle
                | AgentStatus::Running
                | AgentStatus::Waiting
                | AgentStatus::Stopped
                | AgentStatus::Interrupted => {}
            }
            if let Some(observation) = observer.next().await? {
                streaming.observe(observation, agent, started);
            } else {
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
    while let Some(observation) = observer.try_next()? {
        streaming.observe(observation, agent, started);
    }
    let named_after = inspected_name.map(|name| inspect_named_value(&session, agent, name));
    let close = session.close().await;
    if let Err(failure) = &close {
        error = Some(failure.to_string());
    }
    let error = error.or_else(|| snapshot.as_ref().and_then(|state| state.last_error.clone()));
    let mut after = 0;
    let mut facts = BTreeMap::<&str, u64>::new();
    let mut settings = Vec::new();
    let mut reports = Vec::new();
    let mut workspace_outcomes = Vec::new();
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
                EventPayload::ToolResult {
                    conversation,
                    structured,
                    tool_call_id,
                    turn,
                    ..
                } if conversation.as_deref().unwrap_or("main") == agent => {
                    let selected = structured.as_ref().filter(|_| inspected_name.is_some());
                    if let Some(outcome) = workspace_outcome(selected)? {
                        workspace_outcomes.push(json!({
                            "turn": turn, "tool_call_id": tool_call_id, "facts": outcome
                        }));
                    }
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
    let mut result = json!({"session_id":session.id(),"agent":snapshot,"agents":session.agents(),"events_path":session.events_path(),"last_event_id":after,"facts":facts,"provider_settings":settings,"provider_reports":reports,"streaming":streaming,"limits":{"max_requests":limits.max_requests_per_open,"max_agents":limits.max_agents,"max_output_tokens":limits.max_output_tokens_per_request,"max_request_bytes":limits.max_request_bytes,"max_command_seconds":limits.max_command_seconds,"max_context_tokens":max_context_tokens,"automatic_retries":5 * u8::from(limits.retry_transient_provider_errors)},"elapsed_ms":started.elapsed().as_millis(),"cleanup":{"closed":close.is_ok()},"runtime_completed":passed,"error":error});
    result["process_resources"] = json!(process_resources::sample());
    if inspected_name.is_some() {
        result["workspace"] = json!({"named_before": named_before, "named_after": named_after, "outcomes": workspace_outcomes});
    }
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

fn workspace_outcome(value: Option<&Value>) -> serde_json::Result<Option<Value>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value["kind"].as_str() {
        Some("command_result") => {
            serde_json::to_value(CommandExecutionFacts::from_structured(value)?).map(Some)
        }
        Some("workspace_script_result") => {
            serde_json::to_value(WorkspaceScriptFacts::from_structured(value)?).map(Some)
        }
        _ => Ok(None),
    }
}

fn inspect_named_value(session: &Session, agent: &str, name: &str) -> Value {
    let result = (|| {
        let binding = session.named_value(agent, name)?;
        let page =
            session.read_agent_output(agent, &binding.reference.id, &OutputRead::default())?;
        let bytes: Vec<u8> = page
            .segments
            .iter()
            .flat_map(|segment| segment.as_bytes())
            .copied()
            .collect();
        Ok::<_, kuku::Error>(json!({
            "binding": binding,
            "complete": page.complete && !page.has_more && !page.capture_truncated && !page.cleanup_incomplete,
            "value": serde_json::from_slice::<Value>(&bytes).ok()
        }))
    })();
    result.unwrap_or_else(|error| json!({"error": error.to_string()}))
}

pub(crate) fn arguments(
    mut values: impl Iterator<Item = String>,
) -> Result<BTreeMap<String, String>, AnyError> {
    let mut args = BTreeMap::new();
    while let Some(flag) = values.next() {
        let name = flag
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected positional argument '{flag}'"))?;
        if matches!(
            name,
            "help" | "no-agents" | "no-skills" | "no-provider-retry"
        ) {
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
                | "max-requests"
                | "max-agents"
                | "max-output-tokens"
                | "max-context-tokens"
                | "max-request-bytes"
                | "max-command-seconds"
                | "timeout"
                | "command-id"
                | "prompts-dir"
                | "inspect-named-value"
                | "seed-named-json"
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
    Ok(optional_number(args, name)?.unwrap_or(default))
}

fn optional_number<T: std::str::FromStr>(
    args: &BTreeMap<String, String>,
    name: &str,
) -> Result<Option<T>, AnyError> {
    args.get(name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("invalid numeric value for '--{name}'").into())
        })
        .transpose()
}
