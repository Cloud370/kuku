mod common;
#[path = "session_jobs/control.rs"]
mod control;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};
use tempfile::TempDir;

use kuku::event::{EventPayload, StoredEvent};
use kuku::execution::{CommandExecutionFacts, CommandTermination};
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    AgentConfig, AgentStatus, Input, JobStatus, Observation, OutputRead, OutputStream,
    RuntimeLimits, Session, SessionObserver, SessionOptions, WorkspaceScriptFacts,
    WorkspaceScriptOperation,
};

const AGENT: &str = "main";
const CALL: &str = "run-fixture";
const FIRST_TASK: &str = "Run the gated verification fixture.";
const STEERING: &str = "Continue with my new request while the verification runs.";
const ACKNOWLEDGED: &str = "Acknowledged by the model fixture.";
const SHORT_FOREGROUND_MS: u64 = 1500;
const TEST_WAIT: Duration = Duration::from_secs(10);
const STDOUT: &str = "{\"checks\":2,\"fixture\":\"attempt\"}\n";
const STDERR: &str = concat!(
    "test tests::fixture ... FAILED\r\n",
    "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"
);

#[cfg(windows)]
const GATE_FILE: &str = "fixture.cmd";
#[cfg(windows)]
const GATE_SCRIPT: &str = concat!(
    "@echo off\r\n",
    ">>attempts.txt echo started\r\n",
    "set /a ticks=0 >nul\r\n",
    ":wait\r\n",
    "if exist release.txt goto emit\r\n",
    "if %ticks% geq 30 exit /b 98\r\n",
    "set /a ticks+=1 >nul\r\n",
    "\"%SystemRoot%\\System32\\ping.exe\" -n 2 127.0.0.1 >nul 2>&1\r\n",
    "goto wait\r\n",
    ":emit\r\n",
    "type stdout.txt\r\n",
    "type stderr.txt 1>&2\r\n",
    ">completed.txt echo completed\r\n",
    "exit /b 7\r\n"
);

#[cfg(unix)]
const GATE_FILE: &str = "fixture.sh";
#[cfg(unix)]
const GATE_SCRIPT: &str = concat!(
    "printf '%s\\n' started >> attempts.txt\n",
    "ticks=0\n",
    "while [ ! -f release.txt ]; do\n",
    "  [ \"$ticks\" -lt 1500 ] || exit 98\n",
    "  ticks=$((ticks+1))\n",
    "  sleep 0.02\n",
    "done\n",
    "cat stdout.txt\n",
    "cat stderr.txt >&2\n",
    "printf '%s\\n' completed > completed.txt\n",
    "exit 7\n"
);

#[derive(Debug, Default)]
struct CommandAuthorizer(Mutex<Vec<ToolAuthorization>>);

impl ToolAuthorizer for CommandAuthorizer {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        if action.tool.name == "run_command" {
            self.0.lock().unwrap().push(action.clone());
        }
        Ok(AuthorizationDecision::Allow)
    }
}

struct Fixture {
    _temporary: TempDir,
    workspace: PathBuf,
    options: SessionOptions,
    authorizer: Arc<CommandAuthorizer>,
}

impl Fixture {
    fn new(server: &MockServer, route: &str, foreground_ms: u64) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace 字面");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join(GATE_FILE), GATE_SCRIPT).unwrap();
        fs::write(workspace.join("stdout.txt"), STDOUT).unwrap();
        fs::write(workspace.join("stderr.txt"), STDERR).unwrap();

        let mut config = common::test_config();
        config.providers.get_mut("openai").unwrap().base_url = server.url(route);
        let tier = config.tiers.get_mut("balanced").unwrap();
        tier.provider = "openai".into();
        tier.model = "session-jobs".into();
        tier.think = kuku::config::ThinkLevel::Off;
        tier.max_output_tokens = None;
        config.plugin.enabled = false;
        config.handoff.enabled = false;

        let mut options = SessionOptions::new(&workspace, temporary.path().join("data"), config);
        options.limits.job_foreground_wait_ms = foreground_ms;
        options.limits.workspace_script.timeout_ms = 700;
        options.limits.max_requests_per_open = Some(6);
        options.limits.retry_transient_provider_errors = false;
        let authorizer = Arc::new(CommandAuthorizer::default());
        options.authorizer = authorizer.clone();
        Self {
            _temporary: temporary,
            workspace,
            options,
            authorizer,
        }
    }

    async fn open(&self) -> Session {
        let session = Session::open(self.options.clone()).await.unwrap();
        session
            .create_agent("create-main", AGENT, AgentConfig::default())
            .unwrap();
        session
    }

    fn release(&self) {
        fs::write(self.workspace.join("release.txt"), []).unwrap();
    }

    fn assert_launch(&self) {
        let attempts = fs::read_to_string(self.workspace.join("attempts.txt")).unwrap();
        assert_eq!(vec!["started"], attempts.lines().collect::<Vec<_>>());
        let recorded = self.authorizer.0.lock().unwrap();
        let [authorization] = recorded.as_slice() else {
            panic!("the process should be authorized once: {recorded:?}");
        };
        assert_eq!(AGENT, authorization.agent);
        assert_eq!(
            fs::canonicalize(&self.workspace).unwrap(),
            authorization.workspace
        );
        let expected = command_args(false);
        assert_eq!(expected["program"], authorization.args["program"]);
        assert_eq!(expected["args"], authorization.args["args"]);
        assert!(authorization.args.get("timeout").is_none());
    }
}

fn script_args(background: bool) -> Value {
    let command = command_args(false);
    let code = format!(
        r#"
const run = await tools.run_command({command});
const stdout_lines = [];
for await (const line of run.stdout.lines()) stdout_lines.push(line);
const stderr_lines = [];
for await (const line of run.stderr.lines()) stderr_lines.push(line);
return {{
    exit_code: run.exit_code,
    capture_complete: run.capture_complete,
    output_ref: run.output_ref,
    text: await run.stdout.text(),
    parsed: await run.stdout.json(),
    stdout_lines,
    stderr_lines,
}};"#
    );
    if background {
        json!({"code": code, "background": true})
    } else {
        json!({"code": code})
    }
}

fn command_args(background: bool) -> Value {
    let mut args = if cfg!(windows) {
        common::shell_args(GATE_FILE)
    } else {
        json!({"program":"sh","args":[GATE_FILE]})
    };
    if background {
        args["background"] = true.into();
    }
    args
}

fn first_request(body: &Value) -> bool {
    let messages = body["messages"].as_array().unwrap();
    !messages.iter().any(|message| message["role"] == "tool")
        && messages
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .is_some_and(|message| message["content"].to_string().contains(FIRST_TASK))
}

fn wire_body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap()).unwrap()
}

async fn model_fixture<'a>(
    server: &'a MockServer,
    route: &str,
    tool: &str,
    args: Value,
) -> (Mock<'a>, Mock<'a>) {
    let path = format!("{route}/chat/completions");
    let first = common::openai_sse_response(json!({
        "choices":[{"message":{"tool_calls":[{
            "id":CALL,"type":"function","function":{"name":tool,"arguments":args.to_string()}
        }]},"finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":100,"completion_tokens":20}
    }));
    let continuation = common::openai_sse_response(json!({
        "choices":[{"message":{"content":ACKNOWLEDGED},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":100,"completion_tokens":8}
    }));
    let start = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(|request| first_request(&wire_body(request)))
                .matches(common::capture_http_request);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(first);
        })
        .await;
    let finish = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .matches(|request| !first_request(&wire_body(request)))
                .matches(common::capture_http_request);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(continuation);
        })
        .await;
    (start, finish)
}

async fn committed(
    observer: &mut SessionObserver,
    mut predicate: impl FnMut(&EventPayload) -> bool,
) -> StoredEvent {
    tokio::time::timeout(TEST_WAIT, async {
        loop {
            let update = observer.next().await.unwrap().expect("SDK observer closed");
            if let Observation::Committed(event) = update {
                if predicate(&event.payload) {
                    return *event;
                }
            }
        }
    })
    .await
    .expect("the Session did not publish its committed result")
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(TEST_WAIT, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the fixture condition was not met");
}

async fn first_result(observer: &mut SessionObserver) -> (u64, Value) {
    let event = committed(observer, |payload| {
        matches!(
            payload, EventPayload::ToolResult { tool_call_id, .. } if tool_call_id == CALL
        )
    })
    .await;
    let EventPayload::ToolResult {
        status, structured, ..
    } = event.payload
    else {
        unreachable!();
    };
    assert_eq!("ok", status, "{structured:?}");
    (
        event.id,
        structured.expect("tool result has public structured facts"),
    )
}

fn reported_exit(command: &CommandExecutionFacts) {
    assert_eq!(Some(7), command.exit_code);
    assert_eq!(CommandTermination::Exited, command.termination);
    assert!(command.started && command.capture_complete && !command.cleanup_incomplete);
    let [tests] = command.reported_tests.as_slice() else {
        panic!("missing native runner summary: {command:?}");
    };
    assert_eq!(
        (1, 1, Some("tests::fixture")),
        (tests.passed, tests.failed, tests.first_failure.as_deref())
    );
}

fn assert_script_output(session: &Session, structured: &Value) -> CommandExecutionFacts {
    let script = WorkspaceScriptFacts::from_structured(structured).unwrap();
    assert!(
        !script.timed_out && !script.cleanup_incomplete,
        "{script:?}"
    );
    let commands: Vec<_> = script
        .actions
        .iter()
        .filter(|action| action.tool == WorkspaceScriptOperation::RunCommand)
        .collect();
    assert_eq!(1, commands.len(), "{script:?}");
    let command = CommandExecutionFacts::from_structured(&commands[0].result).unwrap();
    reported_exit(&command);
    assert_eq!(
        json!({"exit_code":7,"capture_complete":true,"output_ref":command.output_ref,
            "text":STDOUT,"parsed":{"checks":2,"fixture":"attempt"},
            "stdout_lines":[STDOUT.trim_end()],"stderr_lines":STDERR.lines().collect::<Vec<_>>()}),
        script.value
    );
    let page = session
        .read_agent_output(AGENT, &command.output_ref.id, &OutputRead::default())
        .unwrap();
    assert!(page.complete && !page.has_more && !page.capture_truncated);
    let captured = |stream| {
        page.segments
            .iter()
            .filter(|segment| segment.stream == stream)
            .flat_map(|segment| segment.as_bytes())
            .copied()
            .collect::<Vec<_>>()
    };
    assert_eq!(STDOUT.as_bytes(), captured(OutputStream::Stdout));
    assert_eq!(STDERR.as_bytes(), captured(OutputStream::Stderr));
    command
}

fn assert_one_capture(session: &Session) {
    let outputs = session.events_path().parent().unwrap().join("outputs");
    let count = fs::read_dir(outputs)
        .unwrap()
        .filter(|file| {
            file.as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "output")
        })
        .count();
    assert_eq!(1, count);
}

fn job_id(result: &Value) -> &str {
    assert_eq!("job", result["kind"], "{result}");
    result["job_id"]
        .as_str()
        .expect("job tool result must expose its identity")
}

fn job_completed_event(payload: &EventPayload, job_id: &str) -> bool {
    matches!(payload, EventPayload::Unknown(value) if value["kind"] == "runtime.operation"
        && value["record"]["operation"] == "job_finished" && value["record"]["job_id"] == job_id)
}

fn assert_one_completion(session: &Session, job_id: &str) -> String {
    let events = session.events(0, 512).unwrap();
    let matches: Vec<_> = events
        .iter()
        .filter(|event| job_completed_event(&event.payload, job_id))
        .collect();
    assert_eq!(1, matches.len());
    let EventPayload::Unknown(value) = &matches[0].payload else {
        unreachable!();
    };
    assert_eq!(AGENT, value["record"]["delivery"]["target"]);
    assert_eq!(job_id, value["record"]["delivery"]["correlation"]);
    assert_eq!(1, events.iter().filter(|event| matches!(
        &event.payload, EventPayload::ToolResult { tool_call_id, .. } if tool_call_id == CALL
    )).count());
    value["record"]["delivery"]["text"].as_str().unwrap().into()
}

fn submit(session: &Session) {
    session
        .send(Input::new("fixture-input", AGENT, FIRST_TASK))
        .unwrap();
}

fn wire_requests(route: &str) -> Vec<Value> {
    common::take_http_requests(&format!("{route}/chat/completions"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn same_script_keeps_one_command_capture_across_foreground_and_automatic_job() {
    assert_eq!(10_000, RuntimeLimits::default().job_foreground_wait_ms);
    let server = MockServer::start_async().await;
    let script = script_args(false);
    let fast = Fixture::new(&server, "/job-fast", 10_000);
    fast.release();
    let (fast_start, fast_end) =
        model_fixture(&server, "/job-fast", "run_js", script.clone()).await;
    let fast_session = fast.open().await;
    let mut fast_observer = fast_session.observe(0);
    submit(&fast_session);
    let (_, outcome) = first_result(&mut fast_observer).await;
    let fast_command = assert_script_output(&fast_session, &outcome);
    fast.assert_launch();
    assert_one_capture(&fast_session);
    assert!(fast_session.jobs(AGENT).unwrap().is_empty());
    wait_until(|| fast_session.agent(AGENT).unwrap().status == AgentStatus::Idle).await;
    fast_session.close().await.unwrap();
    fast_start.assert_hits_async(1).await;
    fast_end.assert_hits_async(1).await;
    let requests = wire_requests("/job-fast");
    check_first_request(requests.iter().find(|body| first_request(body)).unwrap());

    let slow = Fixture::new(&server, "/job-slow", SHORT_FOREGROUND_MS);
    let (slow_start, slow_end) = model_fixture(&server, "/job-slow", "run_js", script).await;
    let slow_session = slow.open().await;
    let mut observer = slow_session.observe(0);
    submit(&slow_session);
    let (start_cursor, receipt) = first_result(&mut observer).await;
    let id = job_id(&receipt);
    let started = slow_session.job(AGENT, id).unwrap();
    assert_eq!(JobStatus::Running, started.status);
    assert_eq!(1, slow_session.agent(AGENT).unwrap().running_jobs);
    assert_eq!(start_cursor, started.started_at);
    assert_eq!(CALL, started.tool_call_id);
    assert!(started.result.is_none() && !slow.workspace.join("completed.txt").exists());
    assert_eq!(id, slow_session.jobs(AGENT).unwrap()[0].id);
    wait_until(|| slow.workspace.join("attempts.txt").is_file()).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    slow_start.assert_hits_async(1).await;
    slow_end.assert_hits_async(0).await;

    slow_session
        .send(Input::new("steering", AGENT, STEERING))
        .unwrap();
    wait_until(|| slow_session.agent(AGENT).unwrap().last_output.as_deref() == Some(ACKNOWLEDGED))
        .await;
    assert_eq!(
        JobStatus::Running,
        slow_session.job(AGENT, id).unwrap().status
    );
    slow_end.assert_hits_async(1).await;
    assert!(wire_requests("/job-slow")
        .iter()
        .any(|body| body["messages"].to_string().contains(STEERING)));

    slow.release();
    let completion = committed(&mut observer, |event| job_completed_event(event, id)).await;
    let completed = slow_session.job(AGENT, id).unwrap();
    assert_eq!(JobStatus::Completed, completed.status);
    assert_eq!(Some(completion.id), completed.finished_at);
    let result = completed.result.unwrap();
    let command = assert_script_output(&slow_session, result.structured.as_ref().unwrap());
    assert_eq!(fast_command.summary_lines, command.summary_lines);
    assert_eq!(fast_command.reported_tests, command.reported_tests);
    slow.assert_launch();
    assert_one_capture(&slow_session);
    assert_one_completion(&slow_session, id);
    wait_until(|| {
        let agent = slow_session.agent(AGENT).unwrap();
        agent.usage.model_requests == 3 && agent.status == AgentStatus::Idle
    })
    .await;
    slow_end.assert_hits_async(2).await;
    let delivered = wire_requests("/job-slow");
    let wake = delivered
        .iter()
        .find_map(|body| {
            body["messages"].as_array()?.iter().find_map(|message| {
                message["content"]
                    .as_str()
                    .filter(|text| text.starts_with("<kuku_job_result>"))
            })
        })
        .expect("the Job result should reach the model as marked feedback");
    assert!(wake.ends_with("</kuku_job_result>"));
    assert!(wake.contains(&format!("Job {id} (run_js): Completed.")));
    let agent = slow_session.agent(AGENT).unwrap();
    assert_eq!(0, agent.running_jobs);
    assert_eq!(Some(STEERING), agent.admitted_task.as_deref());
    let job_input = slow_session.events(0, 512).unwrap().into_iter().find(|event| matches!(
        &event.payload, EventPayload::MessageUser { job_delivery: true, text, .. } if text == wake
    )).expect("the durable event should identify the Job delivery");
    assert!(slow_session
        .rewind_message("rewind-job", AGENT, job_input.id, wake, false)
        .is_err());
    assert_eq!(
        1,
        delivered
            .iter()
            .filter(|body| body["messages"]
                .to_string()
                .contains(&format!("Job {id} (")))
            .count()
    );
    slow_session.close().await.unwrap();
}

#[tokio::test]
async fn failed_job_delivery_keeps_the_retained_output_id_when_truncated() {
    let server = MockServer::start_async().await;
    let mut fixture = Fixture::new(&server, "/job-output", SHORT_FOREGROUND_MS);
    fixture.options.limits.max_preview_bytes = 1024;
    let stdout = "x".repeat(1_000);
    fs::write(fixture.workspace.join("stdout.txt"), &stdout).unwrap();
    fs::write(fixture.workspace.join("stderr.txt"), "").unwrap();
    let args = command_args(true);
    let (start, _finish) = model_fixture(&server, "/job-output", "run_command", args).await;
    let session = fixture.open().await;
    let mut observer = session.observe(0);
    submit(&session);
    let (_, receipt) = first_result(&mut observer).await;
    let id = job_id(&receipt);
    fixture.release();
    let job = session.wait_job(AGENT, id).await.unwrap();
    assert_eq!(JobStatus::Failed, job.status);
    let output_id = job.output_id.unwrap();
    wait_until(|| session.agent(AGENT).unwrap().status == AgentStatus::Idle).await;
    start.assert_hits_async(1).await;
    let wire = wire_requests("/job-output");
    let delivery = wire
        .iter()
        .flat_map(|body| body["messages"].as_array().unwrap())
        .filter_map(|message| message["content"].as_str())
        .find(|text| text.starts_with("<kuku_job_result>"))
        .expect("model should receive the Job delivery");
    assert!(delivery.contains("Job preview truncated."));
    assert!(delivery.contains(&format!("Output: {output_id} ")));
    assert!(delivery.contains(&format!("Job {id} (run_command): Failed. command exited 7")));
    assert_eq!(1, delivery.matches("command exited 7").count());
    session.close().await.unwrap();
}

#[tokio::test]
async fn fast_background_failure_returns_the_command_result_directly() {
    let server = MockServer::start_async().await;
    let route = "/job-fast-failure";
    let fixture = Fixture::new(&server, route, SHORT_FOREGROUND_MS);
    fixture.release();
    let (start, finish) = model_fixture(&server, route, "run_command", command_args(true)).await;
    let session = fixture.open().await;
    let mut observer = session.observe(0);
    submit(&session);
    let (_, result) = first_result(&mut observer).await;
    reported_exit(&CommandExecutionFacts::from_structured(&result).unwrap());
    assert!(session.jobs(AGENT).unwrap().is_empty());
    wait_until(|| session.agent(AGENT).unwrap().status == AgentStatus::Idle).await;
    assert_eq!(2, wire_requests(route).len());
    start.assert_hits_async(1).await;
    finish.assert_hits_async(1).await;
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn host_process_deadline_applies_to_direct_and_script_commands() {
    let server = MockServer::start_async().await;
    for (route, tool, args) in [
        ("/deadline-command", "run_command", command_args(false)),
        ("/deadline-script", "run_js", script_args(false)),
    ] {
        let mut fixture = Fixture::new(&server, route, 20_000);
        fixture.options.limits.max_command_seconds = std::num::NonZeroU32::new(1);
        let (model_start, model_end) = model_fixture(&server, route, tool, args).await;
        let session = fixture.open().await;
        let mut observer = session.observe(0);
        submit(&session);
        let event = committed(&mut observer, |payload| {
            matches!(payload, EventPayload::ToolResult { tool_call_id, .. } if tool_call_id == CALL)
        })
        .await;
        let EventPayload::ToolResult {
            structured: Some(facts),
            ..
        } = event.payload
        else {
            panic!("expected structured deadline outcome");
        };
        let script;
        let command = if tool == "run_js" {
            script = WorkspaceScriptFacts::from_structured(&facts).unwrap();
            let [action] = script.actions.as_slice() else {
                panic!("expected one process, got {:?}", script.actions);
            };
            &action.result
        } else {
            &facts
        };
        let command = CommandExecutionFacts::from_structured(command).unwrap();
        assert_eq!(CommandTermination::TimedOut, command.termination);
        assert!(command.started && !command.cleanup_incomplete);
        assert!(session.jobs(AGENT).unwrap().is_empty());
        fixture.assert_launch();
        assert!(!fixture.workspace.join("completed.txt").exists());
        wait_until(|| session.agent(AGENT).unwrap().status == AgentStatus::Idle).await;
        model_start.assert_hits_async(1).await;
        model_end.assert_hits_async(1).await;
        session.close().await.unwrap();
        wire_requests(route);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn explicit_background_returns_before_foreground_deadline_and_can_be_cancelled() {
    let server = MockServer::start_async().await;
    let title = "Check fixture in background";
    for (route, tool, mut args) in [
        ("/explicit-command", "run_command", command_args(true)),
        ("/explicit-script", "run_js", script_args(true)),
    ] {
        args["title"] = json!(title);
        let fixture = Fixture::new(&server, route, 20_000);
        let (model_start, model_end) = model_fixture(&server, route, tool, args).await;
        let session = fixture.open().await;
        let mut observer = session.observe(0);
        submit(&session);
        let (start_cursor, receipt) = first_result(&mut observer).await;
        let id = job_id(&receipt);
        assert_eq!("cancel", receipt["on_session_close"]);
        assert!(receipt.get("guidance").is_none());
        let job = session.job(AGENT, id).unwrap();
        assert_eq!(JobStatus::Running, job.status);
        assert_eq!(start_cursor, job.started_at);
        assert_eq!(tool, job.tool);
        assert_eq!(title, job.title);
        assert_eq!(format!("{id} running: {title}"), job.summary);
        let running_ms = job.elapsed_ms.unwrap();
        assert!(running_ms >= 1_000);
        wait_until(|| fixture.workspace.join("attempts.txt").is_file()).await;
        wait_until(|| session.agent(AGENT).unwrap().last_output.as_deref() == Some(ACKNOWLEDGED))
            .await;
        model_start.assert_hits_async(1).await;
        model_end.assert_hits_async(1).await;

        let cancelled = session.cancel_job(AGENT, id).await.unwrap();
        assert_eq!(JobStatus::Cancelled, cancelled.status);
        assert_eq!(title, cancelled.title);
        let cancelled_ms = cancelled.elapsed_ms.unwrap();
        assert!(cancelled_ms >= running_ms);
        let result = cancelled.result.unwrap();
        assert!(!result.cleanup_incomplete);
        if tool == "run_command" {
            let command =
                CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap())
                    .unwrap();
            assert_eq!(CommandTermination::Cancelled, command.termination);
            assert_eq!(
                job.output_id.as_deref(),
                Some(command.output_ref.id.as_str())
            );
        }
        let completion = committed(&mut observer, |event| job_completed_event(event, id)).await;
        assert_eq!(Some(completion.id), cancelled.finished_at);
        assert_eq!(JobStatus::Cancelled, session.job(AGENT, id).unwrap().status);
        fixture.release();
        fixture.assert_launch();
        session.close().await.unwrap();
        assert!(!fixture.workspace.join("completed.txt").exists());
        assert_one_completion(&session, id);

        let mut options = fixture.options.clone();
        options.session_id = Some(session.id().into());
        let reopened = Session::open(options).await.unwrap();
        let job = reopened.job(AGENT, id).unwrap();
        assert_eq!(JobStatus::Cancelled, job.status);
        assert_eq!(title, job.title);
        assert_eq!(Some(cancelled_ms), job.elapsed_ms);
        assert_one_completion(&reopened, id);
        reopened.close().await.unwrap();
        let requests = wire_requests(route);
        let continuation = requests.iter().find(|body| !first_request(body)).unwrap();
        let tool_result = continuation["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "tool" && message["tool_call_id"] == CALL)
            .expect("model continuation carries the running Job receipt");
        let facts: Value = serde_json::from_str(tool_result["content"].as_str().unwrap()).unwrap();
        assert_eq!(id, facts["job_id"]);
        assert_eq!("cancel", facts["on_session_close"]);
        assert_eq!(
            facts["guidance"],
            format!(
                "Call wait_job({}) when the next step needs this Job's finished result.",
                json!({"job_id": id}),
            )
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn detached_script_keeps_its_original_output_during_zero_retention_collection() {
    let server = MockServer::start_async().await;
    let route = "/job-retention";
    let mut fixture = Fixture::new(&server, route, 20_000);
    fixture.options.limits.output_retention_secs = 0;
    let stdout = "captured original output\n".repeat(256);
    fs::write(fixture.workspace.join("stdout.txt"), &stdout).unwrap();
    let command = command_args(false);
    let code = format!(
        r#"
const command = await tools.run_command({command});
const draft = await tools.create_output({{}});
await tools.append_output({{id:draft.result.id, text:"independent derived output"}});
const derived = await tools.finish_output({{id:draft.result.id}});
const text = await command.stdout.text();
return {{length:text.length, original:command.output_ref, derived:derived.result.output_ref}};"#
    );
    let (first, continuation) = model_fixture(
        &server,
        route,
        "run_js",
        json!({"code":code,"background":true}),
    )
    .await;
    let session = fixture.open().await;
    let mut observer = session.observe(0);
    submit(&session);
    let (_, published) = first_result(&mut observer).await;
    let id = job_id(&published);
    wait_until(|| session.agent(AGENT).unwrap().status == AgentStatus::Idle).await;
    assert_eq!(JobStatus::Running, session.job(AGENT, id).unwrap().status);
    fixture.release();
    committed(&mut observer, |event| job_completed_event(event, id)).await;
    let completed = session.job(AGENT, id).unwrap();
    assert_eq!(JobStatus::Completed, completed.status, "{completed:?}");
    let result = completed.result.unwrap();
    let facts = WorkspaceScriptFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
    assert_eq!(
        stdout.len(),
        facts.value["length"].as_u64().unwrap() as usize
    );
    let original = facts.value["original"]["id"].as_str().unwrap();
    let derived = facts.value["derived"]["id"].as_str().unwrap();
    assert_ne!(original, derived);
    let verified = session
        .read_agent_output(
            AGENT,
            original,
            &OutputRead {
                limit: stdout.len() + STDERR.len(),
                ..Default::default()
            },
        )
        .unwrap();
    let retained_stdout: Vec<_> = verified
        .segments
        .iter()
        .filter(|entry| entry.stream == OutputStream::Stdout)
        .flat_map(|entry| entry.as_bytes())
        .copied()
        .collect();
    assert_eq!(stdout.as_bytes(), retained_stdout);
    assert!(facts
        .actions
        .iter()
        .any(|action| action.tool == WorkspaceScriptOperation::ReadOutput
            && action.result["output_id"] == original));
    fixture.assert_launch();
    assert_one_completion(&session, id);
    wait_until(|| {
        let state = session.agent(AGENT).unwrap();
        state.status == AgentStatus::Idle && state.usage.model_requests == 3
    })
    .await;
    first.assert_hits_async(1).await;
    continuation.assert_hits_async(2).await;
    session.close().await.unwrap();
    wire_requests(route);
}

fn check_first_request(body: &Value) {
    let tools = body["tools"].as_array().unwrap();
    let tool = |name| {
        &tools
            .iter()
            .find(|entry| entry["function"]["name"] == name)
            .unwrap_or_else(|| panic!("first request is missing {name}"))["function"]
    };
    let command = tool("run_command");
    assert!(command["description"]
        .as_str()
        .unwrap()
        .contains("The host controls process deadlines."));
    assert_eq!(
        "boolean",
        command["parameters"]["properties"]["background"]["type"]
    );
    assert_eq!(".", command["parameters"]["properties"]["cwd"]["default"]);
    assert_eq!(
        json!([]),
        command["parameters"]["properties"]["args"]["default"]
    );
    assert_eq!(json!(["program"]), command["parameters"]["required"]);
    let fields = command["parameters"]["properties"].as_object().unwrap();
    assert!(fields.keys().map(String::as_str).eq([
        "args",
        "background",
        "cwd",
        "program",
        "title"
    ]));
    let script = tool("run_js");
    assert_eq!(
        "boolean",
        script["parameters"]["properties"]["background"]["type"]
    );
    assert_eq!(json!(["code"]), script["parameters"]["required"]);
    assert_eq!(
        "string",
        script["parameters"]["properties"]["title"]["type"]
    );
    for name in ["list_jobs", "wait_job", "cancel_job"] {
        assert_eq!("object", tool(name)["parameters"]["type"]);
    }
    let description = script["description"].as_str().unwrap();
    for contract in [
        "tools.run_command(",
        "Set background:true on the outer run_js call",
        "Set title on the outer run_js call",
        "The host controls process deadlines.",
        "exit_code",
        "stdout",
        "stderr",
        "duration_ms",
        "capture_complete",
        "output_ref",
        "stdout:await r.stdout.text()",
        "r.result.raw_text : r.error",
        ".json()",
        ".lines()",
        "{status,result,truncated,error?}",
    ] {
        assert!(
            description.contains(contract),
            "first model request does not declare {contract}"
        );
    }
    for (name, arguments, fields) in [
        (
            "find_files",
            &["pattern", "max_depth"][..],
            &["entries"][..],
        ),
        (
            "read_file",
            &["path", "offset", "limit"],
            &["raw_text", "content_hash"],
        ),
        (
            "search_text",
            &["pattern", "view"],
            &["matches", "next_offset"],
        ),
        (
            "edit_file",
            &["old_text", "new_text"],
            &["replacement_count"],
        ),
        (
            "write_file",
            &["path", "content"],
            &["content_hash_after", "bytes_written"],
        ),
        ("stat_file", &["path", "version"], &["exists", "entry_kind"]),
        ("remove_file", &["path"], &["bytes_removed"]),
        (
            "read_output",
            &["id", "name", "offset"],
            &["segments", "next_offset"],
        ),
        ("output_chunks", &[], &["pages"]),
        ("output_lines", &["stream", "offset"], &["line"]),
        (
            "save_value",
            &["name", "output_id", "value"],
            &["reference"],
        ),
        ("list_values", &["prefix", "after"], &["values"]),
        ("create_output", &[], &["id"]),
        ("append_output", &["id", "text"], &["submitted_bytes"]),
        (
            "finish_output",
            &["id"],
            &["output_ref", "capture_complete"],
        ),
    ] {
        assert_script_signature(description, name, arguments, fields);
    }
    assert!(body.get("max_tokens").is_none() && body.get("max_completion_tokens").is_none());
}

fn assert_script_signature(description: &str, name: &str, arguments: &[&str], fields: &[&str]) {
    let signature = format!("{name}(");
    let advertised = description.split(&signature).skip(1).any(|method| {
        let Some((actual_arguments, explanation)) = method.split_once(')') else {
            return false;
        };
        let result = explanation.split(". ").next().unwrap_or(explanation);
        arguments.iter().all(|name| actual_arguments.contains(name))
            && fields.iter().all(|name| result.contains(name))
    });
    assert!(
        advertised,
        "first model request lacks {name} arguments {arguments:?} and result {fields:?}"
    );
}

#[derive(Debug)]
struct DeferredWrite;

impl ToolAuthorizer for DeferredWrite {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        Ok(
            if action.tool.name == "write_file" && action.args["path"] == "guarded.txt" {
                AuthorizationDecision::Defer
            } else {
                AuthorizationDecision::Allow
            },
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_script_stops_at_a_deferred_action_and_keeps_completed_writes() {
    let server = MockServer::start_async().await;
    let route = "/job-deferred-write";
    let mut fixture = Fixture::new(&server, route, 20_000);
    fixture.options.authorizer = Arc::new(DeferredWrite);
    let command = command_args(false);
    let code = format!(
        r#"
await tools.write_file({{path:"prior.txt",content:"completed"}});
await tools.run_command({command});
await tools.write_file({{path:"guarded.txt",content:"needs approval"}});
await tools.write_file({{path:"later.txt",content:"do not dispatch"}});
"#
    );
    let (start, _finish) = model_fixture(
        &server,
        route,
        "run_js",
        json!({"code": code, "background": true}),
    )
    .await;
    let session = fixture.open().await;
    let mut observer = session.observe(0);
    submit(&session);
    let (_, receipt) = first_result(&mut observer).await;
    let id = job_id(&receipt);
    fixture.release();
    let job = session.wait_job(AGENT, id).await.unwrap();
    assert_eq!(JobStatus::Failed, job.status);
    let result = job.result.unwrap();
    assert_eq!("blocked", result.status);
    let facts = WorkspaceScriptFacts::from_structured(&result.structured.unwrap()).unwrap();
    assert_eq!(WorkspaceScriptOperation::WriteFile, facts.actions[0].tool);
    assert_eq!(
        "completed",
        fs::read_to_string(fixture.workspace.join("prior.txt")).unwrap()
    );
    assert!(!fixture.workspace.join("guarded.txt").exists());
    assert!(!fixture.workspace.join("later.txt").exists());
    assert!(session.agent(AGENT).unwrap().permission.is_none());
    let delivery = assert_one_completion(&session, id);
    assert!(delivery.contains("\"path\":\"prior.txt\""), "{delivery}");
    assert!(delivery.contains("\"exit_code\":7"), "{delivery}");
    assert!(
        delivery.contains("read_output({id, offset, limit})"),
        "{delivery}"
    );
    assert!(!delivery.contains("\"path\":\"guarded.txt\""), "{delivery}");
    session.close().await.unwrap();
    start.assert_hits_async(1).await;
    wire_requests(route);
}
