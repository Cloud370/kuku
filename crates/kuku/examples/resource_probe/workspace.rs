use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use httpmock::{Method::POST, Mock, MockServer};
use kuku::event::EventPayload;
use kuku::execution::{CommandExecutionFacts, CommandTermination};
use kuku::{
    Input, NamedValue, NamedValueSource, OutputRead, OutputStream, Receipt, Session,
    SessionOptions, WorkspaceExecutionHost, WorkspaceScriptActionStatus, WorkspaceScriptFacts,
    WorkspaceScriptOperation,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    agent_config, allocator_usage, micros, native_call, number, phase, response, statistics,
    tool_response, wait_idle, AnyError, SAMPLE_WINDOW,
};

const DIRECT: &str = "resource-direct";
const FILE: &str = "resource-file";
const SCRIPT: &str = "resource-script";
const PRESSURE: &str = "resource-pressure";
pub(super) const CONNECTIONS: [&str; 4] = [DIRECT, FILE, SCRIPT, PRESSURE];
const MAIN: &str = "main";
const READER: &str = "reader";
const BASELINE: &str = "baseline";
const PROMPT: &str = "Run the native checks and count failed records in their log.";
const FILE_CONTENT: &str = "The fixture is ready.\n";
const PASS: &[u8] = b"workspace check passed .\n";
const FAIL: &[u8] = b"workspace check failed !\n";
const FAILURE_INTERVAL: usize = 37;
const OUTPUT_READ_BYTES: usize = 16 * 1024;
const WORKER_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const HOLD_PROCESS_MS: usize = 350;
const OPERATIONS_PER_SCRIPT: usize = 3;

pub(super) struct Workload {
    repeats: usize,
    workers: usize,
    host_operations: usize,
    pressure_rounds: usize,
    output_quota: usize,
    checks: usize,
}

impl Workload {
    pub(super) fn new(args: &BTreeMap<String, String>, quota: usize) -> Result<Self, AnyError> {
        let repeats = number(args, "repeated-tasks", 6)?;
        let workers = number(args, "script-workers", 2)?;
        let host_operations = number(args, "script-host-operations", 4)?;
        let pressure_rounds = number(args, "pressure-rounds", 1)?;
        if !(2..=16).contains(&repeats)
            || !(1..=4).contains(&workers)
            || !(1..=8).contains(&host_operations)
            || !(1..=4).contains(&pressure_rounds)
        {
            return Err("workspace resource workload requires 2..=16 repeats, 1..=4 workers, 1..=8 host operations and 1..=4 pressure rounds".into());
        }
        let output_quota = quota.min(1024 * 1024);
        Ok(Self {
            repeats,
            workers,
            host_operations,
            pressure_rounds,
            output_quota,
            checks: output_quota / 3 / PASS.len(),
        })
    }

    pub(super) fn host(&self) -> kuku::Result<WorkspaceExecutionHost> {
        WorkspaceExecutionHost::new(self.workers, self.host_operations)
    }

    pub(super) async fn measure(
        &self,
        server: &MockServer,
        options: &SessionOptions,
        phase_file: &Path,
        report: &mut Value,
    ) -> Result<(), AnyError> {
        let command = native_call("--emit-checks", self.checks)?;
        let script = json!({"code": analysis_script(&command)});
        let pressure =
            json!({"code": pressure_script(&native_call("--hold-process", HOLD_PROCESS_MS)?)});
        let mut sample_file = tempfile::Builder::new()
            .prefix("resource-input-")
            .suffix(".txt")
            .tempfile_in(&options.workspace)?;
        sample_file.write_all(FILE_CONTENT.as_bytes())?;
        let name = sample_file
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("resource input name is not UTF-8")?;
        let read = json!({"path": name});
        let workflows = [
            (DIRECT, "run_command", &command, 1),
            (FILE, "read_file", &read, 1),
            (SCRIPT, "workspace_script", &script, self.repeats + 1),
            (
                PRESSURE,
                "workspace_script",
                &pressure,
                self.pressure_rounds * (self.workers + 1),
            ),
        ];
        let mut mocks = Vec::with_capacity(workflows.len());
        for (connection, tool, args, calls) in workflows {
            mocks.push((
                connection,
                calls,
                fixture(server, connection, tool, args).await,
            ));
        }
        phase(phase_file, "comparison_setup")?;
        report["workspace_workload"] = json!({
            "check_records": self.checks, "expected_analysis": self.expected(),
            "read_window_bytes": OUTPUT_READ_BYTES, "repeated_tasks": self.repeats,
            "script_workers": self.workers, "script_host_operations": self.host_operations,
            "stress_workers_queued": self.workers + 1, "stress_sessions": 2, "stress_rounds": self.pressure_rounds,
            "stress_buffer_bytes_per_worker": WORKER_BUFFER_BYTES,
            "stress_commands_per_worker": OPERATIONS_PER_SCRIPT, "stress_child_hold_ms": HOLD_PROCESS_MS,
            "repeated_session_retained_quota_bytes": self.output_quota, "ordinary_output_retention_secs": 0,
            "idle_window_ms": SAMPLE_WINDOW.as_millis() as u64,
        });
        self.compare(options, phase_file, report).await?;
        let mut hits = BTreeMap::new();
        for (connection, calls, (first, last)) in mocks {
            let observed = [first.hits_async().await, last.hits_async().await];
            if observed != [calls, calls] {
                return Err(format!(
                    "{connection} expected {calls} model requests per step, observed {observed:?}"
                )
                .into());
            }
            hits.insert(connection, observed);
        }
        report["workspace_fixture_requests"] = json!(hits);
        Ok(())
    }

    async fn compare(
        &self,
        options: &SessionOptions,
        phase_file: &Path,
        report: &mut Value,
    ) -> Result<(), AnyError> {
        let mut script_options = options.clone();
        script_options.limits.max_output_bytes = self.output_quota as u64;
        script_options.limits.max_retained_output_bytes = self.output_quota as u64;
        script_options.limits.max_preview_bytes = 1024;
        script_options.limits.output_retention_secs = 0;
        let direct = open_comparison(options.clone(), DIRECT).await?;
        direct.create_agent("create-reader", READER, agent_config(FILE))?;
        let mut script = open_comparison(script_options.clone(), SCRIPT).await?;
        let result = async {
            self.compare_open(options, &direct, &script, phase_file, report)
                .await?;
            phase(phase_file, "quota")?;
            self.reopen_and_check_quota(&mut script, script_options, report)
                .await?;
            phase(phase_file, "script_peak_setup")?;
            self.pressure(options, phase_file, report).await?;
            phase(phase_file, "post_task_idle")?;
            tokio::time::sleep(SAMPLE_WINDOW).await;
            report["workspace_allocator"]["post_task_idle"] = allocator_usage();
            Ok::<_, AnyError>(())
        }
        .await;
        let (direct_closed, script_closed) = tokio::join!(direct.close(), script.close());
        result?;
        direct_closed?;
        script_closed?;
        Ok(())
    }

    async fn compare_open(
        &self,
        options: &SessionOptions,
        direct: &Session,
        script: &Session,
        phase_file: &Path,
        report: &mut Value,
    ) -> Result<(), AnyError> {
        phase(phase_file, "direct_file")?;
        report["workspace_comparison"]["direct_file"] = run_file_read(direct).await?;
        phase(phase_file, "direct_work")?;
        let expected = self.expected();
        report["workspace_comparison"]["direct"] = run_direct(direct, expected).await?;
        let before_script = options.workspace_host.metrics();
        report["workspace_comparison"]["workers_before_script"] = json!(before_script);
        if before_script.started_workers != 0 || before_script.active_workers != 0 {
            return Err(
                "ordinary conversation or direct commands launched workspace workers".into(),
            );
        }
        phase(phase_file, "post_direct_idle")?;
        tokio::time::sleep(SAMPLE_WINDOW).await;
        report["workspace_allocator"]["post_direct_idle"] = allocator_usage();
        phase(phase_file, "script_first")?;
        let (first, baseline_id) = run_script(script, expected, 0).await?;
        report["workspace_comparison"]["script_first"] = first;
        let workers_after_first = options.workspace_host.metrics();
        report["workspace_comparison"]["workers_after_first"] = json!(workers_after_first);
        if workers_after_first.started_workers != 1 || workers_after_first.active_workers != 0 {
            return Err(format!(
                "first script did not start and release one worker: {workers_after_first:?}"
            )
            .into());
        }
        let before_name = disk_usage(script)?;
        let baseline = bind_baseline(script, &baseline_id).await?;
        let after_name = disk_usage(script)?;
        if before_name.output_bytes != after_name.output_bytes || after_name.output_files != 1 {
            return Err("naming the baseline duplicated output data".into());
        }
        report["workspace_comparison"]["named_baseline"] = json!(baseline);
        phase(phase_file, "script_repeated")?;
        self.repeat_scripts(
            script,
            expected,
            baseline.reference.bytes,
            after_name,
            report,
        )
        .await?;
        let workers_after_repeats = options.workspace_host.metrics();
        report["workspace_comparison"]["workers_after_repeats"] = json!(workers_after_repeats);
        if workers_after_repeats.started_workers != 1 + self.repeats
            || workers_after_repeats.active_workers != 0
        {
            return Err(format!(
                "repeated scripts did not release their workers: {workers_after_repeats:?}"
            )
            .into());
        }
        phase(phase_file, "post_analysis_idle")?;
        tokio::time::sleep(SAMPLE_WINDOW).await;
        report["workspace_allocator"]["post_analysis_idle"] = allocator_usage();
        Ok(())
    }

    async fn repeat_scripts(
        &self,
        session: &Session,
        expected: Analysis,
        baseline_bytes: u64,
        initial_disk: DiskUsage,
        report: &mut Value,
    ) -> Result<(), AnyError> {
        let mut snapshots = vec![initial_disk];
        let mut turn_us = Vec::with_capacity(self.repeats);
        let mut prior = None;
        for index in 1..=self.repeats {
            let (sample, output_id) = run_script(session, expected, index).await?;
            turn_us.push(sample["turn_us"].as_u64().ok_or("script latency missing")?);
            let disk = disk_usage(session)?;
            if disk.output_files != 2
                || disk.output_bytes > self.output_quota as u64
                || disk.output_bytes <= baseline_bytes
            {
                return Err(format!(
                    "repeated script {index} retained unexpected output: {disk:?}"
                )
                .into());
            }
            if let Some(id) = prior.replace(output_id) {
                if !matches!(
                    session.read_agent_output(MAIN, &id, &OutputRead::default()),
                    Err(kuku::Error::OutputUnavailable(_))
                ) {
                    return Err(format!("unreferenced prior output {id} remained available past the configured expiry").into());
                }
            }
            snapshots.push(disk);
        }
        report["workspace_comparison"]["script_repeated_us"] = statistics(&turn_us);
        report["workspace_comparison"]["repeated_disk"] = json!(snapshots);
        Ok(())
    }

    async fn reopen_and_check_quota(
        &self,
        session: &mut Session,
        mut options: SessionOptions,
        report: &mut Value,
    ) -> Result<(), AnyError> {
        let before = session.named_value(MAIN, BASELINE)?;
        session.close().await?;
        options.session_id = Some(SCRIPT.into());
        let start = Instant::now();
        *session = Session::open(options).await?;
        let reopened_us = micros(start);
        let reopened = session.named_value(MAIN, BASELINE)?;
        let reopened_disk = disk_usage(session)?;
        if reopened != before || reopened_disk.output_files != 1 {
            return Err(format!(
                "reopen did not retain exactly the named baseline: {reopened_disk:?}"
            )
            .into());
        }
        let requested = self.output_quota * 3 / 4;
        let attempt = session
            .save_named_value(
                MAIN,
                BASELINE,
                NamedValueSource::Bytes(vec![b'x'; requested]),
                true,
            )
            .await;
        let Err(kuku::Error::OutputQuotaExceeded { available }) = attempt else {
            return Err(format!("over-quota baseline replacement returned {attempt:?}").into());
        };
        if available != self.output_quota as u64 - reopened_disk.output_bytes {
            return Err(format!(
                "replacement reported the wrong remaining output quota: {available}"
            )
            .into());
        }
        let after_disk = disk_usage(session)?;
        let mut hash = Sha256::new();
        let decoded = count_output(session, &before.reference.id, Some(&mut hash))?;
        let sha256 = format!("{:x}", hash.finalize());
        if reopened_disk != after_disk
            || session.named_values(MAIN)? != [before.clone()]
            || decoded != self.expected()
            || sha256 != expected_digest(self.checks)
        {
            return Err(
                "failed replacement changed the retained baseline or its on-disk bytes".into(),
            );
        }
        report["workspace_comparison"]["quota_and_reopen"] = json!({
            "events_path": session.events_path(), "reopen_us": reopened_us, "disk_after_reopen": reopened_disk,
            "replace_requested_bytes": requested, "available_quota_bytes": available,
            "baseline_unchanged_after_rejection": true, "baseline_sha256": sha256,
        });
        Ok(())
    }

    async fn pressure(
        &self,
        options: &SessionOptions,
        phase_file: &Path,
        report: &mut Value,
    ) -> Result<(), AnyError> {
        let mut options = options.clone();
        options.limits.max_output_bytes = 4096;
        options.limits.max_agents = self.workers + 1;
        options.limits.max_model_requests = self.workers + 1;
        options.session_id = Some("resource-pressure-a".into());
        let first = Session::open(options.clone()).await?;
        options.session_id = Some("resource-pressure-b".into());
        let second = Session::open(options.clone()).await?;
        let result = self
            .run_pressure(&first, &second, &options.workspace_host, phase_file, report)
            .await;
        let (first_closed, second_closed) = tokio::join!(first.close(), second.close());
        first_closed?;
        second_closed?;
        result
    }

    async fn run_pressure(
        &self,
        first: &Session,
        second: &Session,
        host: &WorkspaceExecutionHost,
        phase_file: &Path,
        report: &mut Value,
    ) -> Result<(), AnyError> {
        let scheduled: Vec<_> = (0..=self.workers)
            .map(|index| ([first, second][index % 2], format!("stress-{index}")))
            .collect();
        for (session, agent) in &scheduled {
            session.create_agent(&format!("create-{agent}"), agent, agent_config(PRESSURE))?;
        }
        phase(phase_file, "before_pressure_idle")?;
        tokio::time::sleep(SAMPLE_WINDOW).await;
        report["workspace_allocator"]["before_pressure_idle"] = allocator_usage();
        let mut rounds = Vec::with_capacity(self.pressure_rounds);
        for round in 1..=self.pressure_rounds {
            let peak_phase = if round == 1 {
                "script_peak".into()
            } else {
                format!("script_peak_{round}")
            };
            phase(phase_file, &peak_phase)?;
            let mut result = pressure_round(&scheduled, round).await?;
            let metrics = host.metrics();
            if metrics.peak_workers != self.workers
                || metrics.active_workers != 0
                || metrics.started_workers != 1 + self.repeats + round * (self.workers + 1)
            {
                return Err(format!("sessions did not share and release the configured script worker capacity: {metrics:?}").into());
            }
            result["worker_metrics"] = json!(metrics);
            rounds.push(result);
            let idle_phase = format!("after_pressure_idle_{round}");
            phase(phase_file, &idle_phase)?;
            tokio::time::sleep(SAMPLE_WINDOW).await;
            report["workspace_allocator"][&idle_phase] = allocator_usage();
        }
        report["workspace_comparison"]["shared_host"] = json!({
            "worker_metrics": host.metrics(), "rounds": rounds,
            "events_paths": [first.events_path(), second.events_path()],
        });
        Ok(())
    }

    fn expected(&self) -> Analysis {
        Analysis {
            bytes: (self.checks * PASS.len()) as u64,
            failures: self.checks.div_ceil(FAILURE_INTERVAL) as u64,
        }
    }
}

async fn pressure_round(scheduled: &[(&Session, String)], round: usize) -> Result<Value, AnyError> {
    let start = Instant::now();
    let mut receipts = Vec::with_capacity(scheduled.len());
    for (session, agent) in scheduled {
        let receipt = session.send(Input::new(
            format!("input-{agent}-{round}"),
            agent,
            "Exercise shared workspace capacity.",
        ))?;
        receipts.push((*session, receipt));
    }
    let mut completed = Vec::with_capacity(receipts.len());
    for (session, receipt) in receipts {
        wait_idle(session, &receipt.agent, receipt.event_id).await?;
        let result = tool_result(session, &receipt, PRESSURE)?;
        let facts = script_facts(result.facts)?;
        if facts.value
            != json!({"successful": OPERATIONS_PER_SCRIPT, "buffer_bytes": WORKER_BUFFER_BYTES})
            || facts.actions.len() != OPERATIONS_PER_SCRIPT
        {
            return Err(format!(
                "pressure script {} returned unexpected facts: {facts:?}",
                receipt.agent
            )
            .into());
        }
        for action in facts.actions {
            if action.tool != WorkspaceScriptOperation::RunCommand {
                return Err(
                    format!("pressure script emitted an unexpected action: {action:?}").into(),
                );
            }
            command_facts(&action.result, 0)?;
        }
        completed.push(receipt.agent);
    }
    Ok(json!({"round": round, "completed": completed, "wall_us": micros(start)}))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Analysis {
    bytes: u64,
    failures: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct DiskUsage {
    events_bytes: u64,
    output_bytes: u64,
    output_files: usize,
}

fn disk_usage(session: &Session) -> Result<DiskUsage, AnyError> {
    let events = session.events_path();
    let outputs = events
        .parent()
        .ok_or("session has no directory")?
        .join("outputs");
    let mut usage = DiskUsage {
        events_bytes: fs::metadata(events)?.len(),
        output_bytes: 0,
        output_files: 0,
    };
    for file in fs::read_dir(outputs)? {
        let file = file?;
        if file.file_type()?.is_file() {
            usage.output_files += 1;
            usage.output_bytes += file.metadata()?.len();
        }
    }
    Ok(usage)
}

async fn open_comparison(
    mut options: SessionOptions,
    connection: &str,
) -> Result<Session, AnyError> {
    options.session_id = Some(connection.into());
    options.limits.max_preview_bytes = 1024;
    let session = Session::open(options).await?;
    session.create_agent("create-main", MAIN, agent_config(connection))?;
    Ok(session)
}

async fn bind_baseline(session: &Session, output_id: &str) -> Result<NamedValue, AnyError> {
    let source = NamedValueSource::Output(output_id.into());
    let named = session
        .save_named_value(MAIN, BASELINE, source.clone(), false)
        .await?;
    let once = disk_usage(session)?;
    if session
        .save_named_value(MAIN, BASELINE, source, false)
        .await?
        != named
        || disk_usage(session)? != once
    {
        return Err("saving the same baseline twice changed durable state".into());
    }
    Ok(named)
}

async fn run_file_read(session: &Session) -> Result<Value, AnyError> {
    let start = Instant::now();
    let receipt = session.send(Input::new("read-fixture", READER, "Read the fixture file."))?;
    wait_idle(session, READER, receipt.event_id).await?;
    let turn_us = micros(start);
    let result = tool_result(session, &receipt, FILE)?;
    if result.facts["raw_text"] != FILE_CONTENT || result.facts["is_full_file_snapshot"] != true {
        return Err(format!(
            "direct file read returned unexpected facts: {}",
            result.facts
        )
        .into());
    }
    Ok(
        json!({"turn_us": turn_us, "file_bytes": FILE_CONTENT.len(), "model_evidence_bytes": result.model_content_bytes}),
    )
}

async fn run_direct(session: &Session, expected: Analysis) -> Result<Value, AnyError> {
    let start = Instant::now();
    let receipt = session.send(Input::new("direct", MAIN, PROMPT))?;
    wait_idle(session, MAIN, receipt.event_id).await?;
    let turn_us = micros(start);
    let result = tool_result(session, &receipt, DIRECT)?;
    let facts = command_facts(&result.facts, expected.bytes)?;
    let scan = Instant::now();
    let observed = count_output(session, &facts.output_ref.id, None)?;
    let scan_us = micros(scan);
    if observed != expected {
        return Err(format!("direct analysis expected {expected:?}, got {observed:?}").into());
    }
    Ok(json!({
        "analysis": observed, "turn_us": turn_us, "host_analysis_us": scan_us,
        "total_us": micros(start), "command_duration_ms": facts.duration_ms,
        "model_evidence_bytes": result.model_content_bytes, "request_bytes": result.request_bytes,
        "model_requests": session.agent(MAIN)?.usage.model_requests,
        "events_path": session.events_path(),
    }))
}

async fn run_script(
    session: &Session,
    expected: Analysis,
    index: usize,
) -> Result<(Value, String), AnyError> {
    let start = Instant::now();
    let receipt = session.send(Input::new(format!("analysis-{index}"), MAIN, PROMPT))?;
    wait_idle(session, MAIN, receipt.event_id).await?;
    let turn_us = micros(start);
    let result = tool_result(session, &receipt, SCRIPT)?;
    let facts = script_facts(result.facts)?;
    let observed: Analysis = serde_json::from_value(facts.value)?;
    if observed != expected {
        return Err(format!("script analysis expected {expected:?}, got {observed:?}").into());
    }
    let command_action = facts
        .actions
        .iter()
        .find(|action| action.tool == WorkspaceScriptOperation::RunCommand)
        .ok_or("script did not retain the observed native command facts")?;
    let command = command_facts(&command_action.result, expected.bytes)?;
    if !facts
        .actions
        .iter()
        .any(|action| action.tool == WorkspaceScriptOperation::ReadOutput)
    {
        return Err("script did not read the command's retained output".into());
    }
    Ok((
        json!({
            "analysis": observed, "turn_us": turn_us, "command_duration_ms": command.duration_ms,
            "model_evidence_bytes": result.model_content_bytes, "request_bytes": result.request_bytes,
            "model_requests": session.agent(MAIN)?.usage.model_requests,
        }),
        command.output_ref.id,
    ))
}

struct MeasuredResult {
    facts: Value,
    model_content_bytes: usize,
    request_bytes: Vec<u64>,
}

fn tool_result(session: &Session, receipt: &Receipt, id: &str) -> Result<MeasuredResult, AnyError> {
    let mut cursor = receipt.event_id;
    let mut result = None;
    let mut request_bytes = Vec::new();
    loop {
        let events = session.events(cursor, 128)?;
        if events.is_empty() {
            break;
        }
        for event in events {
            cursor = event.id;
            match event.payload {
                EventPayload::ToolResult {
                    tool_call_id,
                    conversation,
                    status,
                    summary,
                    model_content,
                    structured,
                    ..
                } if tool_call_id == id
                    && conversation.as_deref() == Some(receipt.agent.as_str()) =>
                {
                    if status != "ok" {
                        let detail: String = model_content.chars().take(240).collect();
                        return Err(format!(
                            "tool {id} for {} returned {status}: {summary}: {detail}",
                            receipt.agent
                        )
                        .into());
                    }
                    result = Some((
                        structured.ok_or("tool returned no durable facts")?,
                        model_content.len(),
                    ));
                }
                EventPayload::Unknown(value)
                    if value["kind"] == "runtime.step_prepared"
                        && value["agent"] == receipt.agent =>
                {
                    if let Some(bytes) = value["request_bytes"].as_u64() {
                        request_bytes.push(bytes);
                    }
                }
                _ => {}
            }
        }
    }
    let (facts, model_content_bytes) =
        result.ok_or_else(|| format!("tool {id} for {} has no durable result", receipt.agent))?;
    Ok(MeasuredResult {
        facts,
        model_content_bytes,
        request_bytes,
    })
}

fn script_facts(result: Value) -> Result<WorkspaceScriptFacts, AnyError> {
    let facts = WorkspaceScriptFacts::from_structured(&result)?;
    if facts.timed_out
        || facts.cleanup_incomplete
        || facts.not_dispatched != 0
        || facts
            .actions
            .iter()
            .any(|action| action.status != WorkspaceScriptActionStatus::Ok)
    {
        return Err(format!("script recorded an incomplete host action: {facts:?}").into());
    }
    Ok(facts)
}

fn command_facts(result: &Value, payload_bytes: u64) -> Result<CommandExecutionFacts, AnyError> {
    let command = CommandExecutionFacts::from_structured(result)?;
    if command.termination != CommandTermination::Exited
        || command.exit_code != Some(0)
        || !command.capture_complete
        || command.cleanup_incomplete
        || command.quota_truncated
        || command.stdout_bytes != payload_bytes
        || command.output_ref.bytes != payload_bytes
        || command.stderr_bytes != 0
    {
        return Err(format!("native check run returned unexpected facts: {command:?}").into());
    }
    Ok(command)
}

fn count_output(
    session: &Session,
    id: &str,
    mut hash: Option<&mut Sha256>,
) -> Result<Analysis, AnyError> {
    let mut request = OutputRead {
        offset: 0,
        limit: OUTPUT_READ_BYTES,
        search: None,
    };
    let mut analysis = Analysis {
        bytes: 0,
        failures: 0,
    };
    loop {
        let chunk = session.read_agent_output(MAIN, id, &request)?;
        if !chunk.complete || chunk.capture_truncated || chunk.cleanup_incomplete {
            return Err(format!("retained output {id} is incomplete").into());
        }
        for segment in chunk.segments {
            if segment.stream != OutputStream::Stdout {
                return Err("native check log unexpectedly includes stderr".into());
            }
            let bytes = segment.as_bytes();
            analysis.bytes += bytes.len() as u64;
            analysis.failures += bytes.iter().filter(|byte| **byte == b'!').count() as u64;
            if let Some(hash) = &mut hash {
                hash.update(bytes);
            }
        }
        if !chunk.has_more {
            return Ok(analysis);
        }
        if chunk.next_offset <= request.offset {
            return Err("output cursor did not advance".into());
        }
        request.offset = chunk.next_offset;
    }
}

async fn fixture<'a>(
    server: &'a MockServer,
    connection: &str,
    tool: &str,
    args: &Value,
) -> (Mock<'a>, Mock<'a>) {
    use httpmock::prelude::HttpMockRequest;

    fn is_continuation(request: &HttpMockRequest) -> bool {
        serde_json::from_slice::<Value>(request.body.as_deref().unwrap_or_default())
            .ok()
            .and_then(|body| {
                body["messages"]
                    .as_array()
                    .and_then(|messages| messages.last())
                    .map(|message| message["role"] == "tool")
            })
            .unwrap_or(false)
    }

    let path = format!("/{connection}/chat/completions");
    let first = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(|request| !is_continuation(request));
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(tool_response(connection, tool, args));
        })
        .await;
    let last = server
        .mock_async(|when, then| {
            when.method(POST).path(path).matches(is_continuation);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":"checks inspected"}), "stop"));
        })
        .await;
    (first, last)
}

fn analysis_script(command: &Value) -> String {
    format!(
        r#"
        const command = await workspace.run_command({command});
        if (command.status !== 'ok' || !command.result.capture_complete) throw new Error(command.summary);
        let bytes = 0, failures = 0;
        for await (const chunk of workspace.output_chunks({{id: command.result.output_ref.id, limit: {OUTPUT_READ_BYTES}}})) {{
            for (const part of chunk.segments) {{
                bytes += part.bytes;
                failures += (part.text.match(/!/g) ?? []).length;
            }}
        }}
        return {{bytes, failures}};
    "#
    )
}

fn pressure_script(command: &Value) -> String {
    format!(
        r#"
        const buffer = new Uint8Array({WORKER_BUFFER_BYTES});
        buffer.fill(17);
        const results = await Promise.all(Array.from({{length: {OPERATIONS_PER_SCRIPT}}}, () => workspace.run_command({command})));
        const successful = results.filter(action => action.status === 'ok' && action.result.exit_code === 0).length;
        return {{successful, buffer_bytes: buffer.length * Number(buffer[buffer.length - 1] === 17)}};
    "#
    )
}

pub(super) fn emit_checks(checks: usize) -> Result<(), AnyError> {
    let mut output = BufWriter::new(std::io::stdout().lock());
    for line in check_lines(checks) {
        output.write_all(line)?;
    }
    output.flush()?;
    Ok(())
}

fn expected_digest(checks: usize) -> String {
    let mut hash = Sha256::new();
    for line in check_lines(checks) {
        hash.update(line);
    }
    format!("{:x}", hash.finalize())
}

fn check_lines(checks: usize) -> impl Iterator<Item = &'static [u8]> {
    (0..checks).map(|index| {
        if index % FAILURE_INTERVAL == 0 {
            FAIL
        } else {
            PASS
        }
    })
}
