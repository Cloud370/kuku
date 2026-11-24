use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kuku::event::EventPayload;
use kuku::{
    Activation, AgentConfig, AgentSnapshot, AgentStatus, Input, InputSource, Observation,
    OutputChunk, OutputRead, Receipt, Session, SessionObserver, SessionOptions,
};
use serde::Serialize;
use serde_json::{Value, json};

type AnyError = Box<dyn std::error::Error>;

const WRITER: &str = "writer";
const REVIEWER: &str = "reviewer";
const ROUND_LIMIT: usize = 3;

struct Settings {
    options: SessionOptions,
    output: PathBuf,
    writer: AgentConfig,
    reviewer: AgentConfig,
    prompt: String,
    continuation: String,
    command_id: String,
    timer_ms: u64,
    timeout: Duration,
    max_output_tokens: u32,
    max_context_tokens: u32,
}

impl Settings {
    fn load(args: &BTreeMap<String, String>) -> Result<Self, AnyError> {
        let mut config =
            kuku::config::load_config(&PathBuf::from(required(args, "config")?))?.resolve()?;
        let max_context_tokens = number(args, "max-context-tokens", 131_072_u32)?;
        if max_context_tokens == 0 {
            return Err("--max-context-tokens must be positive".into());
        }
        let max_output_tokens = number(args, "max-output-tokens", 32_768_u32)?;
        for model in config.models.values_mut() {
            model.context_window = model.context_window.min(max_context_tokens);
        }
        config.handoff.enabled = false;
        config.plugin.enabled = false;
        let mut options = SessionOptions::new(
            required(args, "workspace")?,
            required(args, "data-dir")?,
            config,
        );
        options.session_id = args.get("session-id").cloned();
        options.prompts_dir = args.get("prompts-dir").map(PathBuf::from);
        options.limits.max_agents = 2;
        options.limits.max_requests_per_open = Some(number(args, "max-requests", 16_u64)?);
        options.limits.retry_transient_provider_errors = false;
        options.limits.max_request_bytes = args
            .get("max-request-bytes")
            .map(|value| value.parse())
            .transpose()
            .map_err(|_| "invalid numeric value for '--max-request-bytes'")?;
        let mut reviewer = selection(args, REVIEWER, max_output_tokens);
        reviewer.instructions = Some(std::fs::read_to_string(required(
            args,
            "review-prompt-file",
        )?)?);
        Ok(Self {
            options,
            output: PathBuf::from(required(args, "output")?),
            writer: selection(args, WRITER, max_output_tokens),
            reviewer,
            prompt: std::fs::read_to_string(required(args, "prompt-file")?)?,
            continuation: std::fs::read_to_string(required(args, "continue-prompt-file")?)?,
            command_id: args
                .get("command-id")
                .cloned()
                .unwrap_or_else(kuku::session::new_session_id),
            timer_ms: number(args, "timer-ms", 250_u64)?,
            timeout: Duration::from_secs(number(args, "timeout", 300_u64)?),
            max_output_tokens,
            max_context_tokens,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Draft,
    Review,
    TimerContinuation,
}

#[derive(Serialize)]
struct Round {
    phase: Phase,
    #[serde(serialize_with = "serialize_input")]
    input: Input,
    receipt: Receipt,
    repeated_receipt_matches: bool,
    receipt_verified_after_settlement: bool,
    snapshot: AgentSnapshot,
}

#[derive(Default, Serialize)]
struct Observed {
    committed: u64,
    transient: u64,
    lagged: u64,
    cursor: u64,
}

impl Observed {
    fn drain(&mut self, observer: &mut SessionObserver) -> kuku::Result<()> {
        while let Some(observation) = observer.try_next()? {
            match observation {
                Observation::Committed(_) => self.committed += 1,
                Observation::Live { .. } | Observation::ReplyPreview { .. } => self.transient += 1,
                Observation::Lagged { skipped } => self.lagged += skipped,
            }
        }
        self.cursor = observer.cursor();
        Ok(())
    }
}

#[derive(Default, Serialize)]
struct Facts {
    facts: BTreeMap<&'static str, u64>,
    provider_settings: Vec<Value>,
    provider_reports: Vec<Value>,
    command_outputs: Vec<OutputChunk>,
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
        println!(
            "session_orchestrate --workspace DIR --prompt-file FILE --review-prompt-file FILE --continue-prompt-file FILE --config FILE --data-dir DIR --output FILE [--session-id ID --command-id KEY --writer-connection NAME --writer-model MODEL --writer-reasoning-effort VALUE --writer-service-tier VALUE --reviewer-connection NAME --reviewer-model MODEL --reviewer-reasoning-effort VALUE --reviewer-service-tier VALUE --timer-ms 250 --max-requests 16 --max-output-tokens 32768 --max-context-tokens 131072 --max-request-bytes BYTES --timeout 300]"
        );
        return Ok(true);
    }
    let mut settings = Settings::load(&args)?;
    let started = Instant::now();
    let mut session = Session::open(settings.options.clone()).await?;
    settings.options.session_id = Some(session.id().into());
    let mut observed = Observed::default();
    let mut observer = session.observe(0);
    let mut rounds = Vec::with_capacity(ROUND_LIMIT);
    let discussion = discuss(&session, &settings, &mut rounds).await;
    let mut error = discussion.err().map(|error| error.to_string());
    merge_error(&mut error, observed.drain(&mut observer));
    let mut closed = close(&session, &mut error).await;
    merge_error(&mut error, observed.drain(&mut observer));
    let mut reopened = false;
    let mut preserved = false;
    let mut timer = Value::Null;
    if error.is_none() && closed {
        let before = session.agents();
        match Session::open(settings.options.clone()).await {
            Ok(next) => {
                session = next;
                reopened = true;
                preserved = before == session.agents();
                observer = session.observe(observed.cursor);
                if preserved {
                    let result = continue_after_timer(&session, &settings, &mut rounds).await;
                    match result {
                        Ok(result) => timer = result,
                        Err(failure) => error = Some(failure.to_string()),
                    }
                } else {
                    error = Some("reopening changed the settled agent snapshots".into());
                }
                merge_error(&mut error, observed.drain(&mut observer));
                closed = close(&session, &mut error).await;
                merge_error(&mut error, observed.drain(&mut observer));
            }
            Err(failure) => error = Some(failure.to_string()),
        }
    }
    let completed =
        error.is_none() && closed && reopened && preserved && rounds.len() == ROUND_LIMIT;
    let result = report(
        &session,
        &settings,
        json!({"round_limit":ROUND_LIMIT,"rounds":rounds,"reopen":{"opened":reopened,"preserved_snapshots":preserved},"timer":timer,"observation":observed}),
        closed,
        completed,
        error,
        started,
    )?;
    let completed = result["runtime_completed"] == true;
    if let Some(parent) = settings
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&settings.output, serde_json::to_vec_pretty(&result)?)?;
    println!(
        "{}",
        json!({"session_id":session.id(),"output":settings.output,"runtime_completed":completed,"cleanup_closed":closed})
    );
    Ok(completed)
}

async fn discuss(
    session: &Session,
    settings: &Settings,
    rounds: &mut Vec<Round>,
) -> Result<(), AnyError> {
    session.create_agent("orchestrate:create:writer", WRITER, settings.writer.clone())?;
    session.create_agent(
        "orchestrate:create:reviewer",
        REVIEWER,
        settings.reviewer.clone(),
    )?;
    let mut draft = Input::new(
        format!("{}:draft", settings.command_id),
        WRITER,
        &settings.prompt,
    );
    draft.source = InputSource::Host("orchestrator".into());
    draft.activation = Activation::Enqueue;
    draft.correlation = Some(settings.command_id.clone());
    rounds.push(run_round(session, draft, Phase::Draft, settings.timeout).await?);
    let writer = &rounds[0].snapshot;
    let mut review = Input::new(
        format!("{}:review", settings.command_id),
        REVIEWER,
        selected_material(writer)?.to_string(),
    );
    review.source = InputSource::Agent(WRITER.into());
    review.activation = Activation::Enqueue;
    review.correlation = Some(settings.command_id.clone());
    rounds.push(run_round(session, review, Phase::Review, settings.timeout).await?);
    Ok(())
}

async fn continue_after_timer(
    session: &Session,
    settings: &Settings,
    rounds: &mut Vec<Round>,
) -> Result<Value, AnyError> {
    let before = requests(session);
    let waited = Instant::now();
    tokio::time::sleep(Duration::from_millis(settings.timer_ms)).await;
    let after = requests(session);
    if before != after {
        return Err("dormant agents dispatched model requests before the timer input".into());
    }
    let waited_ms = waited.elapsed().as_millis();
    let mut input = Input::new(
        format!("{}:timer", settings.command_id),
        WRITER,
        json!({"brief":settings.continuation,"selected_review":selected_material(&session.agent(REVIEWER)?)?}).to_string(),
    );
    input.source = InputSource::External("timer".into());
    input.correlation = Some(settings.command_id.clone());
    rounds.push(run_round(session, input, Phase::TimerContinuation, settings.timeout).await?);
    Ok(
        json!({"delay_ms":settings.timer_ms,"waited_ms":waited_ms,"requests_before":before,"requests_after_wait":after,"source":InputSource::External("timer".into()),"correlation":settings.command_id}),
    )
}

async fn run_round(
    session: &Session,
    input: Input,
    phase: Phase,
    timeout: Duration,
) -> Result<Round, AnyError> {
    let receipt = session.send(input.clone())?;
    let repeated = session.send(input.clone())?;
    if repeated != receipt {
        return Err(format!("input '{}' changed its durable receipt", input.command_id).into());
    }
    if input.activation == Activation::Enqueue {
        let state = session.agent(&input.target)?;
        if state.status != AgentStatus::Idle || state.pending_inputs != 1 {
            return Err(format!(
                "agent '{}' advanced before explicit activation",
                input.target
            )
            .into());
        }
        session.advance(&input.target)?;
    }
    let snapshot = tokio::time::timeout(timeout, settled(session, &input.target))
        .await
        .map_err(|_| format!("agent '{}' exceeded {}s", input.target, timeout.as_secs()))??;
    if session.input_receipt(&input)?.as_ref() != Some(&receipt) {
        return Err(format!("input '{}' lost its settled receipt", input.command_id).into());
    }
    Ok(Round {
        phase,
        input,
        receipt,
        repeated_receipt_matches: true,
        receipt_verified_after_settlement: true,
        snapshot,
    })
}

async fn settled(session: &Session, address: &str) -> Result<AgentSnapshot, AnyError> {
    loop {
        let snapshot = session.agent(address)?;
        match snapshot.status {
            AgentStatus::Idle if snapshot.pending_inputs == 0 => return Ok(snapshot),
            AgentStatus::Waiting | AgentStatus::Stopped | AgentStatus::Interrupted => {
                return Err(format!(
                    "agent '{address}' settled as {:?}: {:?}",
                    snapshot.status, snapshot.last_error
                )
                .into());
            }
            AgentStatus::Idle | AgentStatus::Running => {}
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn selected_material(snapshot: &AgentSnapshot) -> Result<Value, AnyError> {
    let text = snapshot.last_output.as_ref().ok_or_else(|| {
        format!(
            "agent '{}' has no durable result to share",
            snapshot.address
        )
    })?;
    Ok(json!({"agent":snapshot.address,"turn":snapshot.turn,"text":text}))
}

fn serialize_input<S: serde::Serializer>(input: &Input, serializer: S) -> Result<S::Ok, S::Error> {
    json!({"command_id":input.command_id,"source":input.source,"target":input.target,"activation":input.activation,"correlation":input.correlation,"text_bytes":input.text.len()}).serialize(serializer)
}

fn requests(session: &Session) -> u64 {
    session
        .agents()
        .iter()
        .map(|agent| agent.usage.model_requests)
        .sum()
}

async fn close(session: &Session, error: &mut Option<String>) -> bool {
    match session.close().await {
        Ok(()) => true,
        Err(failure) => {
            error.get_or_insert_with(|| failure.to_string());
            false
        }
    }
}

fn merge_error(error: &mut Option<String>, result: kuku::Result<()>) {
    if let Err(failure) = result {
        error.get_or_insert_with(|| failure.to_string());
    }
}

fn report(
    session: &Session,
    settings: &Settings,
    orchestration: Value,
    closed: bool,
    completed: bool,
    mut error: Option<String>,
    started: Instant,
) -> Result<Value, AnyError> {
    let (last_event_id, mut evidence) = facts(session)?;
    let primary = evidence.remove(WRITER).unwrap_or_default();
    let agents = session.agents();
    let persisted = Session::read_agents(session.events_path())?;
    let persisted_matches = persisted == agents;
    if !persisted_matches {
        error
            .get_or_insert_with(|| "closed agent snapshots differ from their durable facts".into());
    }
    let limits = &settings.options.limits;
    Ok(
        json!({"session_id":session.id(),"agent":session.agent(WRITER).ok(),"agents":agents,"events_path":session.events_path(),"last_event_id":last_event_id,"facts":primary.facts,"provider_settings":primary.provider_settings,"provider_reports":primary.provider_reports,"command_outputs":primary.command_outputs,"other_agent_evidence":evidence,"orchestration":orchestration,"limits":{"max_agents":limits.max_agents,"max_requests_per_open":limits.max_requests_per_open,"max_opens":2,"max_output_tokens":settings.max_output_tokens,"max_context_tokens":settings.max_context_tokens,"max_request_bytes":limits.max_request_bytes,"automatic_retries":0},"elapsed_ms":started.elapsed().as_millis(),"cleanup":{"closed":closed,"persisted_snapshots_match":persisted_matches},"runtime_completed":completed && persisted_matches,"error":error}),
    )
}

fn facts(session: &Session) -> Result<(u64, BTreeMap<String, Facts>), AnyError> {
    let mut cursor = 0;
    let mut agents = BTreeMap::<String, Facts>::new();
    loop {
        let events = session.events(cursor, 128)?;
        if events.is_empty() {
            return Ok((cursor, agents));
        }
        for event in events {
            cursor = event.id;
            if let EventPayload::ToolResult {
                conversation,
                structured: Some(value),
                ..
            } = &event.payload
            {
                if value["kind"] == "command_result" {
                    let output_id = value["output_id"]
                        .as_str()
                        .ok_or("command result has no output identity")?;
                    let output = session.read_output(output_id, &OutputRead::default())?;
                    agents
                        .entry(conversation.as_deref().unwrap_or("main").into())
                        .or_default()
                        .command_outputs
                        .push(output);
                }
            }
            let (address, key) = match &event.payload {
                EventPayload::ToolCall { conversation, .. } => {
                    (conversation.as_deref().unwrap_or("main"), "tool_calls")
                }
                EventPayload::ToolResult { conversation, .. } => {
                    (conversation.as_deref().unwrap_or("main"), "tool_results")
                }
                EventPayload::ModelResponse { conversation, .. } => {
                    (conversation.as_deref().unwrap_or("main"), "model_responses")
                }
                EventPayload::TurnCompleted { conversation, .. } => {
                    (conversation.as_str(), "turns_completed")
                }
                EventPayload::Unknown(value) if value["kind"] == "runtime.step_prepared" => {
                    let facts = agents
                        .entry(value["agent"].as_str().unwrap_or("main").into())
                        .or_default();
                    facts.provider_settings.push(value.clone());
                    *facts.facts.entry("prepared_steps").or_default() += 1;
                    continue;
                }
                EventPayload::Unknown(value) if value["kind"] == "runtime.provider_report" => {
                    agents
                        .entry(value["agent"].as_str().unwrap_or("main").into())
                        .or_default()
                        .provider_reports
                        .push(value.clone());
                    continue;
                }
                _ => continue,
            };
            *agents
                .entry(address.into())
                .or_default()
                .facts
                .entry(key)
                .or_default() += 1;
        }
    }
}

fn selection(
    args: &BTreeMap<String, String>,
    address: &str,
    max_output_tokens: u32,
) -> AgentConfig {
    let option = |suffix: &str| args.get(&format!("{address}-{suffix}")).cloned();
    AgentConfig {
        connection: option("connection"),
        model: option("model"),
        reasoning_effort: option("reasoning-effort"),
        service_tier: option("service-tier"),
        max_output_tokens: Some(max_output_tokens),
        no_agents: true,
        no_skills: true,
        ..Default::default()
    }
}

pub(crate) fn arguments(
    values: impl Iterator<Item = String>,
) -> Result<BTreeMap<String, String>, AnyError> {
    let mut values = values;
    let mut args = BTreeMap::new();
    while let Some(flag) = values.next() {
        let name = flag
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument '{flag}'"))?;
        if name == "help" {
            args.insert(name.into(), String::new());
            continue;
        }
        let agent_option = [WRITER, REVIEWER].iter().any(|address| {
            name.strip_prefix(&format!("{address}-"))
                .is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "connection" | "model" | "reasoning-effort" | "service-tier"
                    )
                })
        });
        if !agent_option
            && !matches!(
                name,
                "workspace"
                    | "prompt-file"
                    | "review-prompt-file"
                    | "continue-prompt-file"
                    | "config"
                    | "data-dir"
                    | "output"
                    | "session-id"
                    | "command-id"
                    | "prompts-dir"
                    | "timer-ms"
                    | "max-requests"
                    | "max-output-tokens"
                    | "max-context-tokens"
                    | "max-request-bytes"
                    | "timeout"
            )
        {
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
