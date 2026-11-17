#[path = "../tests/common/mod.rs"]
#[allow(
    dead_code,
    reason = "this caller reuses the integration SSE and configuration fixtures"
)]
mod common;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use httpmock::{Method::POST, MockServer};
use kuku::event::EventPayload;
use kuku::{Activation, AgentConfig, AgentStatus, Input, OutputRead, Session, SessionOptions};
use serde_json::{json, Value};

type AnyError = Box<dyn std::error::Error>;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), AnyError> {
    let args = arguments()?;
    if let Some(bytes) = args.get("emit-output") {
        return emit_output(bytes.parse()?);
    }
    let output = PathBuf::from(required(&args, "output")?);
    let report = probe(&args).await?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string(&report)?);
    if report["passed"] != true {
        std::process::exit(1);
    }
    Ok(())
}

async fn probe(args: &BTreeMap<String, String>) -> Result<Value, AnyError> {
    let workspace = PathBuf::from(required(args, "workspace")?);
    let data = PathBuf::from(required(args, "data-dir")?);
    let phase_file = PathBuf::from(required(args, "phase-file")?);
    let idle_agents = number(args, "idle-agents", 32)?;
    let appends = number(args, "appends", 64)?;
    let workers = number(args, "concurrent-agents", 2)?;
    let output_bytes = number(args, "output-bytes", 4 * 1024 * 1024)?;
    let quota = number(args, "output-quota", 1024 * 1024)?;
    if !(1..=128).contains(&appends)
        || !(1..=8).contains(&workers)
        || idle_agents <= workers
        || idle_agents > 128
        || !(4096..=64 * 1024 * 1024).contains(&quota)
        || output_bytes <= quota
        || output_bytes > 64 * 1024 * 1024
    {
        return Err("resource workload is outside the small probe limits".into());
    }
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&data)?;
    let server = MockServer::start_async().await;
    let context = server
        .mock_async(|when, then| {
            when.method(POST).path("/context/chat/completions");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":"context accepted"}), "stop"));
        })
        .await;
    let command = format!(
        "{} --emit-output {output_bytes}",
        shell_argument(&std::env::current_exe()?)
    );
    let first_output = server.mock_async(|when, then| {
        when.method(POST).path("/output/chat/completions").matches(no_tool_results);
        then.status(200).header("content-type", "text/event-stream")
            .body(response(json!({"tool_calls":[{"id":"large-output","type":"function","function":{"name":"run_command","arguments":json!({"command":command,"timeout":30,"brief":"measure bounded output"}).to_string()}}]}), "tool_calls"));
    }).await;
    let last_output = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/output/chat/completions")
                .matches(has_tool_results);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":"output checked"}), "stop"));
        })
        .await;
    let concurrent = server
        .mock_async(|when, then| {
            when.method(POST).path("/concurrent/chat/completions");
            then.status(200)
                .header("content-type", "text/event-stream")
                .delay(Duration::from_millis(250))
                .body(response(json!({"content":"worker completed"}), "stop"));
        })
        .await;
    let mut config = common::test_config();
    let template = config.providers["openai"].clone();
    for provider in config.providers.values_mut() {
        provider.base_url = server.url("/context");
    }
    for connection in ["context", "output", "concurrent"] {
        let mut provider = template.clone();
        provider.base_url = server.url(format!("/{connection}"));
        config.providers.insert(connection.into(), provider);
    }
    let tier = config
        .tiers
        .get_mut("balanced")
        .ok_or("fixture tier missing")?;
    tier.provider = "context".into();
    tier.model = "resource-fixture".into();
    tier.think = kuku::config::ThinkLevel::Off;
    tier.context_window = 131_072;
    tier.max_output_tokens = 32_768;
    config.plugin.enabled = false;
    config.handoff.enabled = false;
    let mut options = SessionOptions::new(workspace, data, config);
    options.session_id = Some("resource-probe".into());
    options.limits.max_agents = idle_agents;
    options.limits.max_model_requests = workers;
    options.limits.max_steps_per_turn = 4;
    options.limits.max_request_bytes = 256 * 1024;
    options.limits.max_output_bytes = quota as u64;
    phase(&phase_file, "fixture_baseline")?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    phase(&phase_file, "cold_open")?;
    let start = Instant::now();
    let mut session = Session::open(options.clone()).await?;
    let mut report = json!({
        "schema_version":1,"paid":false,"models_mocked":true,
        "scope":"public SDK caller including an in-process fixed provider; tools are child processes",
        "build":{"debug_assertions":cfg!(debug_assertions),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"tokio_worker_threads":2},
        "workload":{"idle_agents":idle_agents,"appends":appends,"input_payload_bytes":256,"reopens":5,"output_bytes":output_bytes,"output_quota_bytes":quota,"concurrent_agents":workers,"mock_concurrent_delay_ms":250},
        "measurements":{"cold_initial_open_us":micros(start),"filesystem_cache":"not flushed; reopens use the same committed files"},
        "events_path":session.events_path(),"passed":false
    });
    let result = measure(
        &mut session,
        &options,
        &phase_file,
        idle_agents,
        appends,
        workers,
        &mut report,
    )
    .await;
    phase(&phase_file, "close")?;
    let close = session.close().await;
    report["cleanup_closed"] = json!(close.is_ok());
    report["events_file_bytes"] = json!(std::fs::metadata(session.events_path())?.len());
    report["fixture_requests"] = json!({"context":context.hits_async().await,"output_first":first_output.hits_async().await,"output_continuation":last_output.hits_async().await,"concurrent":concurrent.hits_async().await});
    let selected = context.hits_async().await == 1
        && first_output.hits_async().await == 1
        && last_output.hits_async().await == 1
        && concurrent.hits_async().await == workers;
    report["passed"] = json!(result.is_ok() && close.is_ok() && selected);
    report["error"] = json!(result
        .err()
        .map(|error| error.to_string())
        .or_else(|| close.err().map(|error| error.to_string())));
    phase(&phase_file, "completed")?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    Ok(report)
}

async fn measure(
    session: &mut Session,
    options: &SessionOptions,
    phase_file: &Path,
    idle_agents: usize,
    appends: usize,
    workers: usize,
    report: &mut Value,
) -> Result<(), AnyError> {
    phase(phase_file, "create_agents")?;
    let start = Instant::now();
    session.create_agent("create-main", "main", agent_config("context"))?;
    for index in 1..idle_agents {
        let address = format!("agent-{index}");
        session.create_agent(
            &format!("create-{index}"),
            &address,
            agent_config("context"),
        )?;
    }
    report["measurements"]["create_idle_agents_us"] = json!(micros(start));
    report["idle"] = json!({"agents":session.agents().len(),"running":session.agents().iter().filter(|agent|agent.status==AgentStatus::Running).count(),"model_requests":session.agents().iter().map(|agent|agent.usage.model_requests).sum::<u64>()});
    phase(phase_file, "idle_agents")?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    phase(phase_file, "append")?;
    let mut append_us = Vec::with_capacity(appends);
    for index in 0..appends {
        let mut input = Input::new(
            format!("append-{index}"),
            "main",
            format!("input {index}: {}", "x".repeat(256)),
        );
        input.activation = Activation::Enqueue;
        let start = Instant::now();
        session.send(input)?;
        append_us.push(micros(start));
    }
    report["measurements"]["append_us"] = statistics(&append_us);
    report["pending_before_reopen"] = json!(session.agent("main")?.pending_inputs);
    session.close().await?;
    phase(phase_file, "recovery")?;
    let mut recovery_us = Vec::new();
    for index in 0..5 {
        let start = Instant::now();
        *session = Session::open(options.clone()).await?;
        recovery_us.push(micros(start));
        if session.agent("main")?.pending_inputs != appends {
            return Err("recovery lost pending inputs".into());
        }
        if index != 4 {
            session.close().await?;
        }
    }
    report["measurements"]["reopen_us"] = statistics(&recovery_us);
    phase(phase_file, "context")?;
    let start = Instant::now();
    session.advance("main")?;
    wait_idle(session, "main").await?;
    report["measurements"]["queued_inputs_to_completion_us"] = json!(micros(start));
    phase(phase_file, "large_output")?;
    session.configure_agent("configure-output", "main", agent_config("output"))?;
    let start = Instant::now();
    session.send(Input::new(
        "large-output",
        "main",
        "produce and read bounded command output",
    ))?;
    wait_idle(session, "main").await?;
    report["measurements"]["large_output_turn_us"] = json!(micros(start));
    let mut cursor = 0;
    let mut command_result = None;
    let mut context_us = Vec::new();
    let mut prepare_us = Vec::new();
    let mut context_events = Vec::new();
    loop {
        let events = session.events(cursor, 128)?;
        if events.is_empty() {
            break;
        }
        for event in events {
            cursor = event.id;
            match event.payload {
                EventPayload::ToolResult {
                    structured: Some(value),
                    ..
                } if value["kind"] == "command_result" => command_result = Some(value),
                EventPayload::Unknown(value) if value["kind"] == "runtime.step_prepared" => {
                    if let Some(value) = value["context_prepare_us"].as_u64() {
                        context_us.push(value);
                    }
                    if let Some(value) = value["request_prepare_us"].as_u64() {
                        prepare_us.push(value);
                    }
                    if let Some(value) = value["context_events_read"].as_u64() {
                        context_events.push(value);
                    }
                }
                _ => {}
            }
        }
    }
    let command = command_result.ok_or("large output has no durable tool result")?;
    let output_id = command["output_id"]
        .as_str()
        .ok_or("large output has no retained reference")?;
    let output = session.read_output(
        output_id,
        &OutputRead {
            offset: 0,
            limit: 4096,
            search: None,
        },
    )?;
    let output_bytes = std::fs::metadata(
        session
            .events_path()
            .parent()
            .ok_or("session has no directory")?
            .join("outputs")
            .join(format!("{output_id}.output")),
    )?
    .len();
    if !output.complete
        || !output.capture_truncated
        || output.cleanup_incomplete
        || output_bytes > options.limits.max_output_bytes
    {
        return Err("large output violated its completion, cleanup or storage bound".into());
    }
    report["large_output"] = json!({"observed_stdout_bytes":command["stdout_bytes"],"file_bytes":output_bytes,"retained_payload_bytes":output.total_bytes,"read_bytes":output.segments.iter().map(|segment|segment.bytes).sum::<usize>(),"capture_truncated":output.capture_truncated,"cleanup_incomplete":output.cleanup_incomplete});
    report["measurements"]["context_prepare_us"] = statistics(&context_us);
    report["measurements"]["request_prepare_us"] = statistics(&prepare_us);
    report["measurements"]["context_events_read"] =
        json!({"unit":"events","samples":context_events});
    phase(phase_file, "concurrent_agents")?;
    for index in 1..=workers {
        session.configure_agent(
            &format!("configure-worker-{index}"),
            &format!("agent-{index}"),
            agent_config("concurrent"),
        )?;
    }
    let start = Instant::now();
    for index in 1..=workers {
        session.send(Input::new(
            format!("work-{index}"),
            format!("agent-{index}"),
            "complete the fixed local request",
        ))?;
    }
    report["concurrency"] = json!({"configured_agents":workers,"running_after_send":session.agents().iter().filter(|agent|agent.status==AgentStatus::Running).count(),"model_request_capacity":options.limits.max_model_requests});
    for index in 1..=workers {
        wait_idle(session, &format!("agent-{index}")).await?;
    }
    report["measurements"]["concurrent_agents_wall_us"] = json!(micros(start));
    report["final_agents"] = json!(session.agents().len());
    report["final_pending_inputs"] = json!(session
        .agents()
        .iter()
        .map(|agent| agent.pending_inputs)
        .sum::<usize>());
    Ok(())
}

fn agent_config(connection: &str) -> AgentConfig {
    AgentConfig {
        connection: Some(connection.into()),
        tier: Some("balanced".into()),
        no_agents: true,
        no_skills: true,
        max_output_tokens: Some(32_768),
        ..Default::default()
    }
}

async fn wait_idle(session: &Session, address: &str) -> Result<(), AnyError> {
    tokio::time::timeout(Duration::from_secs(120), async {
        let mut observer = session.observe(0);
        loop {
            let agent = session.agent(address)?;
            if agent.status == AgentStatus::Idle && agent.pending_inputs == 0 {
                return Ok::<_, kuku::Error>(());
            }
            if matches!(
                agent.status,
                AgentStatus::Stopped | AgentStatus::Interrupted | AgentStatus::Waiting
            ) {
                return Err(kuku::Error::InvalidArgument(format!(
                    "resource agent {address} did not complete: {:?}",
                    agent.last_error
                )));
            }
            if observer.next().await?.is_none() {
                return Err(kuku::Error::InvalidEventStream(
                    "resource session closed early".into(),
                ));
            }
        }
    })
    .await??;
    Ok(())
}

fn response(message: Value, reason: &str) -> String {
    common::openai_sse_response(
        json!({"id":"resource-fixture","model":"resource-fixture","choices":[{"message":message,"finish_reason":reason}],"usage":{"prompt_tokens":100,"completion_tokens":20}}),
    )
}

fn no_tool_results(request: &httpmock::prelude::HttpMockRequest) -> bool {
    !has_tool_results(request)
}

fn has_tool_results(request: &httpmock::prelude::HttpMockRequest) -> bool {
    serde_json::from_slice::<Value>(request.body.as_deref().unwrap_or_default())
        .ok()
        .and_then(|body| {
            body["messages"]
                .as_array()
                .map(|messages| messages.iter().any(|message| message["role"] == "tool"))
        })
        .unwrap_or(false)
}

fn phase(path: &Path, name: &str) -> Result<(), AnyError> {
    std::fs::write(path, serde_json::to_vec(&json!({"phase":name}))?)?;
    Ok(())
}

fn statistics(values: &[u64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let percentile = |percent: usize| sorted[(sorted.len() * percent).div_ceil(100) - 1];
    json!({"unit":"us","samples":values,"n":values.len(),"min":sorted[0],"p50":percentile(50),"p95":percentile(95),"max":sorted[sorted.len()-1],"percentile_method":"nearest_rank"})
}

fn micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn emit_output(bytes: usize) -> Result<(), AnyError> {
    let mut stdout = std::io::stdout().lock();
    let chunk = [b'x'; 8192];
    let mut remaining = bytes;
    while remaining > 0 {
        let size = remaining.min(chunk.len());
        stdout.write_all(&chunk[..size])?;
        remaining -= size;
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

#[cfg(windows)]
fn shell_argument(path: &Path) -> String {
    format!("\"{}\"", path.display())
}
#[cfg(not(windows))]
fn shell_argument(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn arguments() -> Result<BTreeMap<String, String>, AnyError> {
    let mut values = std::env::args().skip(1);
    let mut args = BTreeMap::new();
    while let Some(flag) = values.next() {
        let name = flag.strip_prefix("--").ok_or("expected a named argument")?;
        if !matches!(
            name,
            "workspace"
                | "data-dir"
                | "output"
                | "phase-file"
                | "idle-agents"
                | "appends"
                | "concurrent-agents"
                | "output-bytes"
                | "output-quota"
                | "emit-output"
        ) {
            return Err(format!("unknown argument {flag}").into());
        }
        args.insert(name.into(), values.next().ok_or("argument value missing")?);
    }
    Ok(args)
}

fn required<'a>(args: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, AnyError> {
    args.get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("missing --{name}").into())
}
fn number(args: &BTreeMap<String, String>, name: &str, default: usize) -> Result<usize, AnyError> {
    args.get(name)
        .map(|value| value.parse().map_err(Into::into))
        .unwrap_or(Ok(default))
}
