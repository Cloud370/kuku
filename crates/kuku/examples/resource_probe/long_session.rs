#[path = "../support/process_resources.rs"]
mod process_resources;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use kuku::adapter::{AdapterFuture, ModelEvent, ModelRequest, ModelStream};
use kuku::config::ThinkLevel;
use kuku::event::{EventPayload, EventStore, ModelStopReason};
use kuku::execution::CommandExecutionFacts;
use kuku::{AgentConfig, AgentStatus, Input, ModelAdapter, OutputRead, Session, SessionOptions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{common, micros, native_call, number, phase, required, statistics, AnyError};

const SESSION_ID: &str = "long-session-resource-probe";
const JOURNAL_MIB: u64 = 1024 * 1024;
const REQUEST_CAP: usize = 768_000;
const HISTORY_BATCHES: u64 = 9;
const BASELINE: &str = "Inspect command output and retain this rewind marker";
const REOPEN: &str = "Check the persisted workspace after reopen";
const AFTER_UNDO: &str = "Check the preceding checkpoint";
const AFTER_REWIND: &str = "Check the workspace after message rewind";
const HISTORY_SETUP: &str = "Prepare the history lookup tool";
const QUERY_HISTORY: &str = "Recall the most recent verified workspace message";
const FILE_MARKER: &str = "fixture_ingestion_boundary";

#[derive(Debug, Default)]
struct FixtureModel {
    checkpoints: AtomicU64,
    probe_messages: Mutex<Option<Vec<Value>>>,
    inspect_only: bool,
}

impl ModelAdapter for FixtureModel {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            let messages = request.body["messages"].as_array().unwrap();
            let input_tokens =
                (!self.inspect_only).then(|| (request.body.to_string().len() / 2) as u64);
            let last = messages.last().unwrap();
            let prompt = last["content"].as_str().unwrap_or_default();
            let step = request
                .request_id
                .strip_prefix("req_")
                .and_then(|id| id.parse::<u64>().ok())
                .unwrap_or_default();
            let mut events = vec![ModelEvent::StreamStart {
                request_id: request.request_id,
            }];
            let reason = if last["role"] == "user" && prompt.starts_with("Write a checkpoint") {
                let sequence = self.checkpoints.fetch_add(1, Ordering::Relaxed) + 1;
                events.push(ModelEvent::TextDelta {
                    text: format!(
                        "## Delivered and verified\n{}\n{}",
                        checkpoint_marker(sequence),
                        "The ingestion logs have been inspected. Use report.log for the next inspection. The command output is retained for byte-range verification."
                    ),
                });
                ModelStopReason::EndTurn
            } else if self.inspect_only {
                let query_history = messages
                    .iter()
                    .rev()
                    .find(|message| message["role"] == "user")
                    .is_some_and(|message| message["content"] == QUERY_HISTORY);
                if last["role"] == "tool" {
                    let expected = if query_history {
                        HISTORY_SETUP
                    } else {
                        FILE_MARKER
                    };
                    assert!(
                        prompt.contains(expected),
                        "result lacks {expected}: {prompt}"
                    );
                    *self.probe_messages.lock().unwrap() = Some(messages.clone());
                    answer(&mut events)
                } else {
                    let (tool, arguments) = if query_history {
                        ("query_session", json!({"kind":"message.user","limit":2}))
                    } else {
                        (
                            "read_file",
                            json!({"path":"report.log","offset":1,"limit":4}),
                        )
                    };
                    call(&mut events, request.turn, step, 0, tool, arguments);
                    ModelStopReason::ToolUse
                }
            } else if messages
                .iter()
                .rev()
                .find(|message| message["role"] == "user")
                .is_some_and(|message| message["content"] == BASELINE)
            {
                if last["role"] == "user" {
                    call(
                        &mut events,
                        request.turn,
                        step,
                        0,
                        "run_command",
                        native_call("--emit-ingestion-log", 360).unwrap(),
                    );
                    ModelStopReason::ToolUse
                } else {
                    answer(&mut events)
                }
            } else if step > HISTORY_BATCHES {
                answer(&mut events)
            } else {
                reasoning(&mut events, request.turn, step);
                for index in 0..3 {
                    let sample = request.turn * HISTORY_BATCHES * 3 + step * 3 + index;
                    history_call(&mut events, request.turn, step, index, sample);
                }
                ModelStopReason::ToolUse
            };
            events.extend([
                ModelEvent::StopReason { reason },
                ModelEvent::StreamUsage {
                    input_tokens,
                    output_tokens: None,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
                ModelEvent::StreamEnd,
            ]);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

fn checkpoint_marker(sequence: u64) -> String {
    format!("CHECKPOINT_SEQUENCE_{sequence:05}")
}

fn answer(events: &mut Vec<ModelEvent>) -> ModelStopReason {
    events.push(ModelEvent::TextDelta {
        text: "Verified the latest ingestion results and isolated the validation records that need follow-up.".into(),
    });
    ModelStopReason::EndTurn
}

fn reasoning(events: &mut Vec<ModelEvent>, turn: u64, step: u64) {
    let lines = match (turn + step) % 11 {
        0 => 260,
        1..=3 => 70,
        _ => 16,
    };
    let mut text = String::new();
    for row in 0..lines {
        use std::fmt::Write;
        writeln!(
            text,
            "Turn {turn}, batch {step}, record {row}. Verify whether repeated transient import errors share a request ID. Compare the source range with the latest output before reporting a regression."
        )
        .unwrap();
    }
    events.push(ModelEvent::ThinkingDelta { text });
}

fn call(events: &mut Vec<ModelEvent>, turn: u64, step: u64, index: u64, name: &str, args: Value) {
    events.extend([
        ModelEvent::ToolCallStart {
            index,
            id: format!("ingestion-{turn}-{step}-{index}"),
            name: name.into(),
        },
        ModelEvent::ToolCallArgDelta {
            index,
            fragment: args.to_string(),
        },
        ModelEvent::ToolCallStop { index },
    ]);
}

fn history_call(events: &mut Vec<ModelEvent>, turn: u64, step: u64, index: u64, sample: u64) {
    let (name, args) = if sample.is_multiple_of(389) {
        (
            "run_command",
            native_call("--emit-ingestion-log", (sample % 3400 + 1800) as usize).unwrap(),
        )
    } else {
        match sample % 43 {
            0 => (
                "run_command",
                native_call("--emit-ingestion-log", (sample % 170 + 30) as usize).unwrap(),
            ),
            1 => (
                "read_file",
                json!({"path":"report.log","offset":sample % 256 + 1,"limit":14}),
            ),
            _ => {
                let rows = match sample % 17 {
                    0..=3 => 95,
                    4..=7 => 32,
                    _ => 4,
                };
                let code = format!(
                "return Array.from({{length:{rows}}}, (_, i) => `ingestion check ${{i + {sample}}} executed: worker=${{i % 7}} validation=${{i % 11 === 0 ? 'retry' : 'ok'}}  decoded=${{(i + 3) * 43}} bytes`).join('\\n');"
            );
                ("run_js", json!({"code":code}))
            }
        }
    };
    call(events, turn, step, index, name, args);
}

pub(super) fn emit_log(records: usize) -> Result<(), AnyError> {
    let mut stdout = BufWriter::new(std::io::stdout().lock());
    let mut stderr = BufWriter::new(std::io::stderr().lock());
    for row in 0..records {
        if row % 21 == 20 {
            writeln!(
                stderr,
                "WARN ingest worker={} sequence={row}: missing optional source label, queued retry",
                row % 7
            )?;
        } else {
            writeln!(
                stdout,
                "INFO ingest worker={} sequence={row}: decoded {} bytes, checkpoint batch written",
                row % 7,
                400 + (row * 109) % 2513
            )?;
        }
    }
    stdout.flush()?;
    stderr.flush()?;
    Ok(())
}

fn options(
    args: &BTreeMap<String, String>,
    model: Arc<FixtureModel>,
) -> Result<SessionOptions, AnyError> {
    let mut config = common::test_config();
    config.providers.get_mut("openai").unwrap().base_url = "http://127.0.0.1:1".into();
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "long-session-fixture".into();
    tier.think = ThinkLevel::Off;
    tier.context_window = 131_072;
    tier.max_output_tokens = Some(8192);
    config.discovery.auto_discover = false;
    config.plugin.enabled = false;
    config.handoff.enabled = !model.inspect_only;
    let mut options = SessionOptions::new(
        required(args, "workspace")?,
        required(args, "data-dir")?,
        config,
    );
    options.session_id = Some(SESSION_ID.into());
    options.model_adapter = Some(model);
    options.limits.max_request_bytes = Some(REQUEST_CAP);
    options.limits.max_retained_output_bytes = 256 * JOURNAL_MIB;
    Ok(options)
}

#[derive(Serialize, Deserialize)]
struct Fixture {
    events_path: PathBuf,
    journal_bytes: u64,
    #[serde(default)]
    physical_journal_bytes: Option<u64>,
    retained_output_bytes: u64,
    checkpoint_count: u64,
    turn_count: u64,
    rewind_event: u64,
    output_id: String,
    shape: BTreeMap<String, (u64, u64)>,
}

pub(super) async fn generate(args: &BTreeMap<String, String>) -> Result<Value, AnyError> {
    let target_mib = number(args, "long-session-mib", 500)?;
    if !(4..=512).contains(&target_mib) {
        return Err("select a long-session journal between 4 and 512 MiB".into());
    }
    let target = target_mib as u64 * JOURNAL_MIB;
    let phase_file = Path::new(required(args, "phase-file")?);
    let start = Instant::now();
    let model = Arc::new(FixtureModel::default());
    let options = options(args, model.clone())?;
    fs::create_dir_all(&options.workspace)?;
    let report = (0..285)
        .map(|line| format!("{line:03} {FILE_MARKER} ingestion event; decoded payload validated\n"))
        .collect::<String>();
    fs::write(options.workspace.join("report.log"), report)?;
    let session = Session::open(options).await?;
    session.create_agent(
        "create",
        "main",
        AgentConfig {
            no_skills: true,
            no_agents: true,
            tools: Some(vec![
                "read_file".into(),
                "run_js".into(),
                "run_command".into(),
            ]),
            ..Default::default()
        },
    )?;
    let path = session.events_path().to_owned();
    let mut turn = 0;
    let mut next_report = 0;
    loop {
        turn += 1;
        let investigation = [
            "Check the event details around an intermittent ingest failure. Compare decoder logs and flag nearby retries.",
            "Compare the successful worker output with the restarted worker. Validate error boundaries against the input file.",
            "Audit the previous validation sequence and inspect the latest command results before deciding what to retry.",
            "Look for missing report rows around the last import. Check offsets and preserve evidence for later review.",
        ][turn % 4];
        let repetitions = if turn % 13 == 0 { 65 } else { 7 };
        let input = format!(
            "Investigation {turn}. {}",
            investigation.repeat(repetitions)
        );
        send(&session, &format!("investigate-{turn}"), &input).await?;
        let bytes = fs::metadata(&path)?.len();
        if bytes >= next_report {
            fs::write(
                phase_file,
                serde_json::to_vec(&json!({
                    "phase":"generate","journal_bytes":bytes,"turns":turn,
                    "checkpoint_requests":model.checkpoints.load(Ordering::Relaxed),
                    "elapsed_us":micros(start)
                }))?,
            )?;
            next_report = bytes + 8 * JOURNAL_MIB;
        }
        if bytes >= target && model.checkpoints.load(Ordering::Relaxed) >= 2 {
            break;
        }
        if bytes > target + 16 * JOURNAL_MIB {
            return Err("generation exceeded the target before producing two checkpoints".into());
        }
    }
    let old_head = EventStore::read_head(&path)?;
    send(&session, "set-rewind-marker", BASELINE).await?;
    let tail = session.events(old_head, 128)?;
    let rewind_event = tail
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::MessageUser { text, .. } if text == BASELINE => Some(event.id),
            _ => None,
        })
        .ok_or("rewind marker was not committed")?;
    let output_id = tail
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolResult {
                structured: Some(value),
                ..
            } => CommandExecutionFacts::from_structured(value)
                .ok()
                .filter(|facts| facts.exit_code == Some(0))
                .map(|facts| facts.output_ref.id),
            _ => None,
        })
        .ok_or("command did not produce retained output")?;
    let close = Instant::now();
    session.close().await?;
    let close_us = micros(close);
    drop(session);
    let shape = shape(&path)?;
    let checkpoint_count = shape
        .get("checkpoint")
        .map(|value| value.0)
        .unwrap_or_default();
    if checkpoint_count < 2 || checkpoint_count != model.checkpoints.load(Ordering::Relaxed) {
        return Err(format!(
            "expected all checkpoint requests to commit; recorded {checkpoint_count}"
        )
        .into());
    }
    let retained_output_bytes = fs::read_dir(path.parent().unwrap().join("outputs"))?
        .try_fold(0_u64, |size, entry| {
            Ok::<_, std::io::Error>(size + entry?.metadata()?.len())
        })?;
    let fixture = Fixture {
        journal_bytes: shape.values().map(|entry| entry.1).sum(),
        physical_journal_bytes: Some(fs::metadata(&path)?.len()),
        events_path: path,
        retained_output_bytes,
        checkpoint_count,
        turn_count: turn as u64 + 1,
        rewind_event,
        output_id,
        shape,
    };
    Ok(
        json!({"schema_version":1,"passed":true,"paid":false,"generation_us":micros(start),"close_us":close_us,"fixture":fixture}),
    )
}

fn shape(path: &Path) -> Result<BTreeMap<String, (u64, u64)>, AnyError> {
    let mut shape = BTreeMap::<String, (u64, u64)>::new();
    EventStore::visit_committed(path, 0, |event| {
        let raw = serde_json::to_vec(&event)?;
        let bytes = raw.len() + 1;
        let value: Value = serde_json::from_slice(&raw)?;
        let category = match value["kind"].as_str() {
            Some("runtime.compaction_finished") if value["status"] == "completed" => "checkpoint",
            Some("runtime.context_segment") if value["reason"] == "compaction" => {
                "checkpoint_segment"
            }
            Some("message.user") => "user",
            Some("message.assistant") => "assistant",
            Some("runtime.operation") if value["record"]["operation"] == "turn_settled" => {
                "turn_settled"
            }
            Some("tool.call") => "tool_call",
            Some("tool.result") => "tool_result",
            Some("model.response") if value["thinking"].is_string() => "response_with_reasoning",
            Some("runtime.context_admitted") => "context_admission",
            _ => "other",
        };
        let (count, total) = shape.entry(category.into()).or_default();
        *count += 1;
        *total += bytes as u64;
        Ok(true)
    })?;
    Ok(shape)
}

pub(super) async fn prepare(args: &BTreeMap<String, String>) -> Result<Value, AnyError> {
    let options = options(args, Arc::new(FixtureModel::default()))?;
    let phase_file = Path::new(required(args, "phase-file")?);
    let (start, resources) = begin(phase_file, "raw_reopen")?;
    let session = Session::open(options).await?;
    let open = measurement(start, resources);
    let path = session.events_path();
    let before_bytes = fs::metadata(path)?.len();
    let (start, resources) = begin(phase_file, "seal")?;
    session.close().await?;
    let seal = measurement(start, resources);
    let after_bytes = fs::metadata(path)?.len();
    assert!(after_bytes < before_bytes);
    Ok(json!({
        "schema_version":1,"paid":false,"passed":true,"events_path":path,
        "before_bytes":before_bytes,"after_bytes":after_bytes,"open":open,"seal":seal
    }))
}

pub(super) async fn measure(args: &BTreeMap<String, String>) -> Result<Value, AnyError> {
    let report: Value = serde_json::from_reader(File::open(required(args, "fixture")?)?)?;
    let mut fixture: Fixture = serde_json::from_value(report["fixture"].clone())?;
    fixture.physical_journal_bytes = Some(fs::metadata(&fixture.events_path)?.len());
    let phase_file = Path::new(required(args, "phase-file")?);
    let model = Arc::new(FixtureModel {
        inspect_only: true,
        ..Default::default()
    });
    let options = options(args, model.clone())?;
    let mut timings = BTreeMap::new();
    let (start, resources) = begin(phase_file, "reopen")?;
    let session = Session::open(options).await?;
    timings.insert("reopen", measurement(start, resources));
    assert_eq!(fixture.events_path, session.events_path());
    assert_eq!(AgentStatus::Idle, session.agent("main")?.status);
    let (start, resources) = begin(phase_file, "history_tail")?;
    let history = session.events_before(None, 40)?;
    timings.insert("history_tail", measurement(start, resources));
    assert!(history.iter().any(|event| event.id == fixture.rewind_event));

    let (start, resources) = begin(phase_file, "retained_output")?;
    let mut offset = 0;
    let mut output_bytes = 0;
    let mut pages = 0;
    let mut saw_stdout = false;
    let mut saw_stderr = false;
    loop {
        let output = session.read_output(
            &fixture.output_id,
            &OutputRead {
                offset,
                limit: 2048,
                search: None,
            },
        )?;
        assert!(output.complete && !output.capture_truncated && !output.cleanup_incomplete);
        pages += 1;
        for segment in &output.segments {
            use kuku::execution::OutputStream;
            output_bytes += segment.bytes;
            saw_stdout |= segment.stream == OutputStream::Stdout;
            saw_stderr |= segment.stream == OutputStream::Stderr;
        }
        if !output.has_more {
            assert_eq!(output_bytes as u64, output.total_bytes);
            break;
        }
        assert!(output.next_offset > offset);
        offset = output.next_offset;
    }
    timings.insert("retained_output", measurement(start, resources));
    assert!(pages > 1 && output_bytes > 4096 && saw_stdout && saw_stderr);

    let latest = checkpoint_marker(fixture.checkpoint_count);
    let previous = checkpoint_marker(fixture.checkpoint_count - 1);
    let head = EventStore::read_head(session.events_path())?;
    let first = sampled_turn(
        &session,
        phase_file,
        &model,
        "first_turn",
        REOPEN,
        &latest,
        None,
    )
    .await?;
    let mut steady = Vec::new();
    for index in 0..5 {
        steady.push(
            sampled_turn(
                &session,
                phase_file,
                &model,
                "steady_turn",
                &format!("Follow-up validation {index}"),
                &latest,
                None,
            )
            .await?,
        );
    }

    let (start, resources) = begin(phase_file, "undo")?;
    session.restore_context_command("restore-last-checkpoint", "main")?;
    timings.insert("undo", measurement(start, resources));
    let after_undo = sampled_turn(
        &session,
        phase_file,
        &model,
        "after_undo",
        AFTER_UNDO,
        &previous,
        Some(&latest),
    )
    .await?;

    let (start, resources) = begin(phase_file, "rewind")?;
    let (_, warnings) = session.rewind_message(
        "rewind-marker",
        "main",
        fixture.rewind_event,
        BASELINE,
        false,
    )?;
    timings.insert("rewind", measurement(start, resources));
    assert!(warnings.is_empty(), "{warnings:?}");
    let after_rewind = sampled_turn(
        &session,
        phase_file,
        &model,
        "after_rewind",
        AFTER_REWIND,
        &previous,
        Some(BASELINE),
    )
    .await?;

    let mut prepared = Vec::new();
    for event in session.events(head, 512)? {
        if let EventPayload::Unknown(value) = event.payload {
            if value["kind"] == "runtime.step_prepared" {
                prepared.push(json!({
                    "turn":value["turn"],"request":value["request_id"],"bytes":value["request_bytes"],
                    "context_events_read":value["context_events_read"],
                    "context_prepare_us":value["context_prepare_us"],"request_prepare_us":value["request_prepare_us"]
                }));
            }
        }
    }
    assert_eq!(prepared.len(), 16, "expected two steps per measured turn");
    let steady_us: Vec<_> = steady
        .iter()
        .map(|entry| entry["wall_us"].as_u64().unwrap())
        .collect();
    let mut config = session.agent("main")?.config;
    config
        .tools
        .get_or_insert_with(Vec::new)
        .push("query_session".into());
    session.configure_agent("enable-history-probe", "main", config)?;
    phase(phase_file, "history_query_setup")?;
    send(&session, HISTORY_SETUP, HISTORY_SETUP).await?;
    let history_query_turn = sampled_turn(
        &session,
        phase_file,
        &model,
        "history_query",
        QUERY_HISTORY,
        HISTORY_SETUP,
        Some(BASELINE),
    )
    .await?;
    let (start, resources) = begin(phase_file, "close")?;
    session.close().await?;
    timings.insert("close", measurement(start, resources));
    phase(phase_file, "completed")?;
    Ok(json!({
        "schema_version":1,"passed":true,"paid":false,
        "request_cap_bytes":REQUEST_CAP,
        "filesystem_cache":"fresh process; operating system page cache not flushed",
        "fixture":fixture,"timings":timings,"first_turn":first,
        "steady_turns":steady,"steady_turn_wall_us":statistics(&steady_us),
        "after_undo":after_undo,"after_rewind":after_rewind,"history_query_turn":history_query_turn,
        "retained_output_read":{"pages":pages,"bytes":output_bytes},
        "prepared_steps":prepared
    }))
}

async fn send(session: &Session, id: &str, text: &str) -> Result<(), AnyError> {
    session.send(Input::new(id, "main", text))?;
    let end = session.wait_for_agent("main").await?;
    if end.status != AgentStatus::Idle || end.last_error.is_some() {
        return Err(format!("turn {id} ended in {:?}: {:?}", end.status, end.last_error).into());
    }
    Ok(())
}

async fn sampled_turn(
    session: &Session,
    phase_file: &Path,
    model: &FixtureModel,
    label: &str,
    prompt: &str,
    expected: &str,
    forbidden: Option<&str>,
) -> Result<Value, AnyError> {
    let (start, resources) = begin(phase_file, label)?;
    send(session, prompt, prompt).await?;
    let elapsed = measurement(start, resources);
    let messages = model
        .probe_messages
        .lock()
        .unwrap()
        .take()
        .ok_or("missing prepared request")?;
    let wire = serde_json::to_string(&messages)?;
    assert!(wire.contains(expected), "prepared request lacks {expected}");
    if let Some(forbidden) = forbidden {
        assert!(
            !wire.contains(forbidden),
            "prepared request still contains {forbidden}"
        );
    }
    Ok(elapsed)
}

fn begin(phase_file: &Path, label: &str) -> Result<(Instant, Option<Value>), AnyError> {
    phase(phase_file, label)?;
    let resources = process_resources::sample();
    Ok((Instant::now(), resources))
}

fn measurement(start: Instant, before: Option<Value>) -> Value {
    let wall_us = micros(start);
    let after = process_resources::sample();
    let usage = before.zip(after).map(|(start, end)| {
        let seconds = |key| Some(end[key].as_f64()? - start[key].as_f64()?);
        let bytes = |key| Some(end["process_io"][key].as_u64()?.saturating_sub(start["process_io"][key].as_u64()?));
        json!({
            "user_cpu_s":seconds("user_cpu_seconds"),"system_cpu_s":seconds("system_cpu_seconds"),
            "peak_rss_kib_since_process_start":end["peak_rss_kib"],"read_syscall_bytes":bytes("rchar"),
            "filesystem_read_bytes":bytes("read_bytes"),"filesystem_write_bytes":bytes("write_bytes")
        })
    });
    json!({"wall_us":wall_us,"resources":usage})
}
