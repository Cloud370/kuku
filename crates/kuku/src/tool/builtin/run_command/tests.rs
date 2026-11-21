#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use tokio::sync::{mpsc, Notify};
#[cfg(unix)]
use tokio::time::{timeout, Duration};

use crate::execution::{
    CommandExecutionFacts, CommandInvocation, CommandShell, CommandTermination,
};

use super::super::test_helpers::{noisy_timeout_command, workspace};
use super::{
    blocked_command_reason, run_command, run_command_with_output, OutputLimits,
    RUN_COMMAND_MAX_CHARS,
};
#[cfg(unix)]
use super::{read_output, CommandEvent, OutputRead, OutputStream};

#[cfg(unix)]
fn stdout_command() -> &'static str {
    "printf 'hello\nworld\n'"
}

fn shell_name() -> &'static str {
    if cfg!(windows) {
        "cmd"
    } else {
        "sh"
    }
}

#[cfg(windows)]
fn stdout_command() -> &'static str {
    "echo hello && echo world"
}

#[cfg(unix)]
fn stderr_nonzero_command() -> &'static str {
    "printf 'bad\n' >&2; exit 7"
}

#[cfg(windows)]
fn stderr_nonzero_command() -> &'static str {
    "echo bad 1>&2 && exit /B 7"
}

#[cfg(unix)]
fn read_marker_command() -> &'static str {
    "cat cwd-marker.txt"
}

#[cfg(windows)]
fn read_marker_command() -> &'static str {
    "type cwd-marker.txt"
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_captures_stdout_from_workspace() {
    let dir = workspace();
    std::fs::write(dir.path().join("cwd-marker.txt"), "workspace marker\n").unwrap();
    let result = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": read_marker_command(), "timeout": 5, "brief": "read marker"}),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(result.status, "ok", "{result:?}");
    assert!(result.summary.contains("shell exited 0"));
    assert!(result.model_content.contains("stdout:"));
    assert!(result.model_content.contains("workspace marker"));
    let structured = result.structured.unwrap();
    assert_eq!(structured["kind"], "command_result");
    assert_eq!(structured["exit_code"], 0);
    assert_eq!(structured["stdout_lines"], 1);
    assert_eq!(structured["stderr_lines"], 0);
    assert_eq!(structured["timed_out"], false);
    assert_eq!(structured["termination"], "exited");
    assert_eq!(structured["started"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_treats_nonzero_exit_as_command_evidence() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": stderr_nonzero_command(), "timeout": 5, "brief": "test stderr"}),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(result.status, "ok", "{result:?}");
    assert!(result.summary.contains("shell exited 7"));
    assert!(result.model_content.contains("stderr:"));
    assert!(result.model_content.contains("bad"));
    assert_eq!(result.structured.as_ref().unwrap()["exit_code"], 7);
    assert_eq!(result.structured.as_ref().unwrap()["termination"], "exited");
}

#[cfg(unix)]
#[tokio::test]
async fn terminated_process_preserves_the_observed_signal() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"shell":"sh","cwd":".","command":"kill -TERM $$","timeout":5}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!("ok", result.status, "{result:?}");
    let facts =
        CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
    assert_eq!(CommandTermination::Exited, facts.termination);
    assert_eq!(None, facts.exit_code);
    assert_eq!(Some(libc::SIGTERM), facts.signal);
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_reports_timeout_as_tool_error() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": noisy_timeout_command(dir.path()), "timeout": 1, "brief": "test timeout"}),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(result.status, "error", "{result:?}");
    assert!(
        result.summary.contains("command timed out after 1s"),
        "{result:?}"
    );
    assert_eq!(
        result.structured.as_ref().unwrap()["kind"],
        "command_result"
    );
    assert_eq!(result.structured.as_ref().unwrap()["timed_out"], true);
    assert!(result.truncated);
    assert!(result.model_content.len() <= RUN_COMMAND_MAX_CHARS);
    assert!(result.summary.contains("preview truncated"));
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_rejects_invalid_timeout_empty_command_and_dangerous_commands() {
    let dir = workspace();

    let missing = run_command(
        &serde_json::json!({"timeout": 1, "brief": "test"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(missing.status, "error");
    assert!(missing
        .model_content
        .contains("run_command requires program and literal args"));

    let empty = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "   ", "timeout": 1, "brief": "test"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(empty.status, "error");
    assert!(empty.model_content.contains("requires a nonempty command"));

    let zero = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": stdout_command(), "timeout": 0, "brief": "test"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(zero.status, "error");
    assert!(zero.model_content.contains("timeout must be a positive"));

    let over_cap = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": stdout_command(), "timeout": 601, "brief": "test"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(over_cap.structured.as_ref().unwrap()["exit_code"], 0);

    let dangerous = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "git reset --hard HEAD", "timeout": 1, "brief": "test danger"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(dangerous.status, "blocked");
    assert!(dangerous
        .model_content
        .contains("blocked by command hard guard"));
}

#[test]
fn command_hard_guard_does_not_block_git_push_or_gh_pr_create() {
    for text in ["git push origin main", "gh pr create --fill"] {
        let invocation = CommandInvocation::Shell {
            shell: CommandShell::Sh,
            command: text.into(),
        };
        assert_eq!(None, blocked_command_reason(&invocation));
    }
}

#[test]
fn command_hard_guard_still_blocks_destructive_commands() {
    for command in [
        "git reset --hard HEAD",
        "sh -c 'git reset --hard HEAD'",
        "sh -c 'sudo git reset --hard HEAD'",
        "rm -rf target",
        "npm publish",
        "make deploy",
    ] {
        let invocation = CommandInvocation::Shell {
            shell: CommandShell::Sh,
            command: command.into(),
        };
        assert!(
            blocked_command_reason(&invocation).is_some(),
            "{command} should remain hard guarded"
        );
    }
}

#[tokio::test]
async fn native_program_receives_every_literal_argument_in_the_selected_directory() {
    let dir = workspace();
    let working = dir.path().join("项目 with spaces");
    std::fs::create_dir(&working).unwrap();
    let fixture_name = format!("native fixture{}", std::env::consts::EXE_SUFFIX);
    let installed = working.join(&fixture_name);
    let source = std::env::current_exe().unwrap();
    if std::fs::hard_link(&source, &installed).is_err() {
        std::fs::copy(source, &installed).unwrap();
    }
    let literals = [
        "",
        "带 空格",
        "\"double\" and 'single'",
        "left & rm -rf literal",
        "$HOME",
        "a\\",
        "🌱",
    ];
    let mut args = vec![
        "--ignored",
        "--exact",
        "tool::builtin::run_command::tests::argument_fixture",
        "--nocapture",
        "--",
    ];
    args.extend(literals);
    let program = format!("./{fixture_name}");
    let result = run_command(
        &serde_json::json!({"program":program,"args":args,"cwd":"项目 with spaces","timeout":10}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!("ok", result.status, "{result:?}");
    let facts =
        CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
    assert!(facts.started);
    assert_eq!(Some(0), facts.exit_code, "{result:?}");
    assert_eq!(CommandTermination::Exited, facts.termination);
    assert_eq!(
        working.canonicalize().unwrap(),
        std::path::PathBuf::from(&facts.cwd)
    );
    assert_eq!(
        CommandInvocation::Program {
            program,
            args: args.iter().map(|arg| arg.to_string()).collect(),
        },
        facts.invocation
    );
    assert!(
        facts
            .output_preview
            .contains(&serde_json::to_string(&literals).unwrap()),
        "{result:?}"
    );
    assert!(facts.capture_complete);
}

#[test]
#[ignore = "subprocess fixture for literal program arguments"]
fn argument_fixture() {
    let mut stdin = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin).unwrap();
    assert!(stdin.is_empty());
    let args: Vec<_> = std::env::args()
        .skip_while(|arg| arg != "--")
        .skip(1)
        .collect();
    println!("{}", serde_json::to_string(&args).unwrap());
}

#[tokio::test]
async fn native_program_missing_and_invalid_executable_have_distinct_facts() {
    let dir = workspace();
    let invalid = format!("plain text{}", std::env::consts::EXE_SUFFIX);
    let path = dir.path().join(&invalid);
    std::fs::write(&path, b"echo unexpected > interpreted.txt\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (program, termination) in [
        (
            "missing-program-for-kuku-test",
            CommandTermination::ProgramMissing,
        ),
        (invalid.as_str(), CommandTermination::StartFailed),
    ] {
        let program = std::path::Path::new(".").join(program);
        let result = run_command(
            &serde_json::json!({"program":program,"args":[],"cwd":".","timeout":2}),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!("error", result.status, "{result:?}");
        let facts =
            CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
        assert_eq!(termination, facts.termination, "{result:?}");
        assert_eq!(None, facts.exit_code);
        assert!(!facts.started);
        assert!(facts.start_error.is_some());
        assert_eq!(0, facts.output_ref.bytes);
    }
    assert!(!dir.path().join("interpreted.txt").exists());
}

#[tokio::test]
async fn prepared_dispatch_rejects_unresolved_or_mixed_invocations() {
    let dir = workspace();
    for args in [
        serde_json::json!({"command":"echo implicit","cwd":".","timeout":5}),
        serde_json::json!({"program":"echo","args":["mixed"],"shell":"sh","command":"echo mixed","cwd":".","timeout":5}),
        serde_json::json!({"program":"echo","args":["cwd missing"],"timeout":5}),
    ] {
        let result = run_command_with_output(
            &args,
            dir.path(),
            None,
            None,
            &dir.path().join("capture"),
            "unprepared",
            OutputLimits::default(),
        )
        .await;
        assert_eq!("error", result.status, "{result:?}");
        assert_eq!("error", result.structured.unwrap()["kind"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn controlled_path_schema_omits_unavailable_shells_and_explicit_launch_reports_missing() {
    let dir = workspace();
    let decoy = dir.path().join("bash");
    std::fs::write(&decoy, b"#!/bin/sh\necho unavailable\n").unwrap();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .env("PATH", dir.path())
        .args([
            "--ignored",
            "--exact",
            "tool::builtin::run_command::tests::unavailable_shell_fixture",
            "--nocapture",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "subprocess fixture for shell discovery with a controlled PATH"]
async fn unavailable_shell_fixture() {
    let registry = crate::tool::builtin_registry(false, false);
    let tool = registry
        .iter()
        .find(|tool| tool.name == "run_command")
        .unwrap();
    assert!(tool
        .description
        .contains("workspace-relative program with './'"));
    assert!(tool.description.contains("output ID"));
    let schema = &tool.input_schema;
    assert_eq!(serde_json::json!(["program"]), schema["required"]);
    assert!(schema["properties"].get("shell").is_none());
    assert!(schema["properties"].get("command").is_none());

    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"program":"sh","args":["-c","echo should-not-run > marker"],"cwd":".","timeout":5}),
        dir.path(),
        None,
        None,
    ).await;
    let facts =
        CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
    assert_eq!(
        CommandTermination::ProgramMissing,
        facts.termination,
        "{result:?}"
    );
    assert!(!facts.started);
    assert!(!dir.path().join("marker").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_accepts_commands_without_display_labels() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "echo hi", "timeout": 1}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(result.status, "ok");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn run_command_streams_stdout_incrementally() {
    let dir = workspace();
    let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

    // Generate >4KB to reliably trigger the byte threshold flush
    let cmd = "dd if=/dev/zero bs=1 count=5000 2>/dev/null | tr '\\0' 'x'";

    let handle = tokio::spawn(async move {
        run_command(
            &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": cmd, "timeout": 5, "brief": "stream test"}),
            dir.path(),
            Some(tx),
            None,
        )
        .await
    });

    let mut events: Vec<CommandEvent> = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    let result = handle.await.unwrap();
    assert_eq!(result.status, "ok");
    assert!(
        !events.is_empty(),
        "streaming path must emit at least one event"
    );
    for event in &events {
        assert!(
            matches!(event, CommandEvent::Stdout(_)),
            "all events should be stdout, got: {event:?}"
        );
    }
    let streamed: String = events
        .iter()
        .map(|e| match e {
            CommandEvent::Stdout(t) | CommandEvent::Stderr(t) => t.as_str(),
        })
        .collect();
    assert_eq!(streamed.len(), 5000);
    assert!(streamed.chars().all(|c| c == 'x'));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn run_command_streams_stderr_separately() {
    let dir = workspace();
    let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

    // 5000 bytes to stderr triggers byte threshold, not just final flush
    let cmd = "dd if=/dev/zero bs=1 count=5000 2>/dev/null | tr '\\0' 'y' >&2";

    let handle = tokio::spawn(async move {
        run_command(
            &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": cmd, "timeout": 5, "brief": "stderr stream"}),
            dir.path(),
            Some(tx),
            None,
        )
        .await
    });

    let mut events: Vec<CommandEvent> = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    let result = handle.await.unwrap();
    assert_eq!(result.status, "ok");
    assert!(!events.is_empty());
    for event in &events {
        assert!(
            matches!(event, CommandEvent::Stderr(_)),
            "all events should be stderr"
        );
    }
    let streamed: String = events
        .iter()
        .map(|e| match e {
            CommandEvent::Stdout(t) | CommandEvent::Stderr(t) => t.as_str(),
        })
        .collect();
    assert_eq!(streamed.len(), 5000);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn run_command_streams_fast_small_output_via_final_flush() {
    let dir = workspace();
    let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

    // Completes in < 50ms — too fast for the timer, must rely on final flush
    let handle = tokio::spawn(async move {
        run_command(
            &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "echo hello", "timeout": 5, "brief": "fast output"}),
            dir.path(),
            Some(tx),
            None,
        )
        .await
    });

    let mut events: Vec<CommandEvent> = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    let result = handle.await.unwrap();
    assert_eq!(result.status, "ok");
    assert_eq!(events.len(), 1, "fast output should flush exactly once");
    let text = match &events[0] {
        CommandEvent::Stdout(t) => t.clone(),
        _ => panic!("expected stdout"),
    };
    assert!(text.contains("hello"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn run_command_streaming_no_output_produces_no_events() {
    let dir = workspace();
    let (tx, rx) = mpsc::channel::<CommandEvent>(64);

    let result = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "true", "timeout": 5, "brief": "no output"}),
        dir.path(),
        Some(tx),
        None,
    )
    .await;

    assert_eq!(result.status, "ok");
    assert!(rx.is_empty(), "no output should produce no events");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn run_command_streaming_non_utf8_output_is_lossy() {
    let dir = workspace();
    let (tx, mut rx) = mpsc::channel::<CommandEvent>(64);

    // Outputs raw byte 0xFF which is never valid UTF-8
    let cmd = "printf '\\377'";

    let handle = tokio::spawn(async move {
        run_command(
            &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": cmd, "timeout": 5, "brief": "binary output"}),
            dir.path(),
            Some(tx),
            None,
        )
        .await
    });

    let mut events: Vec<CommandEvent> = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    let result = handle.await.unwrap();
    assert_eq!(result.status, "ok");
    // Must not panic from invalid UTF-8; U+FFFD replacement is fine
    assert!(!events.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn large_output_is_persisted_within_quota_without_an_event_consumer() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    let (tx, _rx) = mpsc::channel(1);
    let result = timeout(Duration::from_secs(5), run_command_with_output(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "head -c 2097152 /dev/zero | tr '\\0' x", "timeout": 4, "brief": "large output"}),
        dir.path(), Some(tx), None, &outputs, "large", OutputLimits { max_output_bytes: 8192, max_preview_bytes: 512 },
    )).await.unwrap();
    assert_eq!("ok", result.status);
    assert!(result.truncated);
    assert!(result.model_content.len() <= 512);
    let facts = result.structured.unwrap();
    assert_eq!(2097152, facts["stdout_bytes"]);
    assert_eq!(true, facts["quota_truncated"]);
    assert!(facts["dropped_events"].as_u64().unwrap() > 0);
    assert!(
        std::fs::metadata(outputs.join("large.output"))
            .unwrap()
            .len()
            <= 8192
    );
    let chunk = read_output(
        &outputs,
        "large",
        OutputRead {
            offset: 0,
            limit: 80_000,
            search: None,
        },
    )
    .unwrap();
    assert!(chunk.complete);
    assert!(chunk.capture_truncated);
    assert!(chunk
        .segments
        .iter()
        .all(|segment| segment.text.bytes().all(|byte| byte == b'x')));
}

#[cfg(unix)]
#[tokio::test]
async fn long_failed_command_shows_final_diagnostic_within_preview_bound() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    let diagnostic = "configure terminated: missing required option";
    let result = run_command_with_output(
        &serde_json::json!({
            "shell": shell_name(), "cwd": ".", "timeout": 5, "brief": "fixture build",
            "command": format!("printf '{diagnostic}\\n' >&2; head -c 16384 /dev/zero | tr '\\0' x; exit 7"),
        }),
        dir.path(), None, None, &outputs, "failed-build",
        OutputLimits { max_output_bytes: 32_768, max_preview_bytes: 512 },
    ).await;
    let facts =
        CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
    assert_eq!(facts.exit_code, Some(7));
    assert!(facts.capture_complete);
    assert!(result.model_content.contains("End of captured stderr"));
    assert!(
        result.model_content.contains(diagnostic),
        "{}",
        result.model_content
    );
    assert!(facts.output_preview.contains(diagnostic));
    assert!(facts.preview_truncated);
    assert!(result.model_content.starts_with(&result.summary));
    assert!(result.summary.contains("preview truncated"));
    assert!(!result.summary.contains("capture incomplete"));
    assert!(result.model_content.len() <= 512);
}

#[cfg(unix)]
#[tokio::test]
async fn build_test_summary_survives_the_capture_quota_and_reopen() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    let result = run_command_with_output(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "head -c 131072 /dev/zero | tr '\\0' x; printf '\\ntest parser::quoted ... FAILED\\ntest result: FAILED. 7 passed; 1 failed; 2 ignored\\n'; exit 13", "timeout": 5, "brief": "test summary"}),
        dir.path(), None, None, &outputs, "summary", OutputLimits { max_output_bytes: 8192, max_preview_bytes: 1024 },
    ).await;
    assert_eq!("ok", result.status);
    assert!(result.model_content.contains("7 passed"));
    assert!(result
        .model_content
        .contains("First reported failure: parser::quoted"));
    assert!(result.model_content.contains("Capture quota reached"));
    let facts =
        CommandExecutionFacts::from_structured(result.structured.as_ref().unwrap()).unwrap();
    assert!(facts.output_filtered);
    assert!(facts.quota_truncated);
    assert!(!facts.capture_complete);
    assert_eq!(Some(13), facts.exit_code);
    assert_eq!(1, facts.reported_tests.len());
    assert_eq!(7, facts.reported_tests[0].passed);
    assert_eq!(1, facts.reported_tests[0].failed);
    assert_eq!(2, facts.reported_tests[0].skipped);
    assert_eq!(
        Some("parser::quoted"),
        facts.reported_tests[0].first_failure.as_deref()
    );
    let reopened = read_output(
        &outputs,
        "summary",
        OutputRead {
            offset: 0,
            limit: 100,
            search: None,
        },
    )
    .unwrap();
    assert_eq!(
        vec!["test result: FAILED. 7 passed; 1 failed; 2 ignored".to_string()],
        reopened.summary
    );
}

#[cfg(unix)]
#[tokio::test]
async fn command_persists_the_observed_stdout_stderr_sequence() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    let result = run_command_with_output(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "printf first; sleep 0.05; printf second >&2; sleep 0.05; printf third", "timeout": 5, "brief": "ordered streams"}),
        dir.path(), None, None, &outputs, "ordered", OutputLimits::default(),
    ).await;
    assert_eq!("ok", result.status);
    let chunk = read_output(
        &outputs,
        "ordered",
        OutputRead {
            offset: 0,
            limit: 100,
            search: None,
        },
    )
    .unwrap();
    assert_eq!(
        vec![
            (OutputStream::Stdout, "first"),
            (OutputStream::Stderr, "second"),
            (OutputStream::Stdout, "third")
        ],
        chunk
            .segments
            .iter()
            .map(|segment| (segment.stream, segment.text.as_str()))
            .collect::<Vec<_>>()
    );
}

#[cfg(target_os = "linux")]
async fn wait_for_pid(path: &Path) -> u32 {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(path) {
                if let Ok(pid) = pid.trim().parse() {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[cfg(target_os = "linux")]
fn process_is_running(pid: u32) -> bool {
    const ESRCH: i32 = 3;
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat.rsplit_once(") ").unwrap().1.starts_with('Z'),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(ESRCH) =>
        {
            false
        }
        Err(error) => panic!("could not inspect descendant {pid}: {error}"),
    }
}

#[cfg(target_os = "linux")]
fn process_tree_args(command: &str, timeout: u32, native: bool) -> serde_json::Value {
    if native {
        serde_json::json!({"program":"/bin/sh","args":["-c",command],"cwd":".","timeout":timeout})
    } else {
        serde_json::json!({"shell":shell_name(),"command":command,"cwd":".","timeout":timeout})
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelling_a_command_stops_its_descendant_process() {
    for native in [false, true] {
        let dir = workspace();
        let marker = dir.path().join("child.pid");
        let cancel = Arc::new(Notify::new());
        let worker_cancel = cancel.clone();
        let worker_dir = dir.path().to_path_buf();
        let worker = tokio::spawn(async move {
            run_command(
                &process_tree_args("sleep 30 & echo $! > child.pid; wait", 20, native),
                &worker_dir,
                None,
                Some(worker_cancel),
            )
            .await
        });
        let pid = wait_for_pid(&marker).await;
        assert!(process_is_running(pid));
        cancel.notify_waiters();
        let result = timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!("cancelled", result.status);
        assert!(
            !process_is_running(pid),
            "cancelled command left descendant {pid} running"
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn timeout_stops_descendants_and_dropping_the_command_future_does_too() {
    for native in [false, true] {
        let dir = workspace();
        let worker_dir = dir.path().to_path_buf();
        let worker = tokio::spawn(async move {
            run_command(
                &process_tree_args("sleep 30 & echo $! > timeout.pid; wait", 1, native),
                &worker_dir,
                None,
                None,
            )
            .await
        });
        let pid = wait_for_pid(&dir.path().join("timeout.pid")).await;
        let result = timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(true, result.structured.unwrap()["timed_out"]);
        assert!(!process_is_running(pid));
        let worker_dir = dir.path().to_path_buf();
        let worker = tokio::spawn(async move {
            run_command(
                &process_tree_args("sleep 30 & echo $! > dropped.pid; wait", 20, native),
                &worker_dir,
                None,
                None,
            )
            .await
        });
        let pid = wait_for_pid(&dir.path().join("dropped.pid")).await;
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        timeout(Duration::from_secs(2), async {
            while process_is_running(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn escaped_inherited_pipes_have_a_bounded_cleanup_deadline() {
    struct EscapedProcess(u32);
    impl Drop for EscapedProcess {
        fn drop(&mut self) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", self.0)])
                .status();
        }
    }
    let dir = workspace();
    let worker_dir = dir.path().to_path_buf();
    let worker = tokio::spawn(async move {
        run_command(&serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "setsid sh -c 'echo $$ > detached.pid; sleep 30' & while [ ! -f detached.pid ]; do sleep 0.01; done", "timeout": 5, "brief": "bounded pipe cleanup"}), &worker_dir, None, None).await
    });
    let escaped = EscapedProcess(wait_for_pid(&dir.path().join("detached.pid")).await);
    let result = timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!("ok", result.status);
    assert_eq!(true, result.structured.unwrap()["cleanup_incomplete"]);
    assert!(result.truncated);
    assert!(
        process_is_running(escaped.0),
        "test must exercise a process outside the original group"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_output_storage_and_preexisting_ids_prevent_command_side_effects() {
    let dir = workspace();
    let output_file = dir.path().join("not-a-directory");
    std::fs::write(&output_file, "occupied").unwrap();
    let result = run_command_with_output(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "touch should-not-exist", "timeout": 5, "brief": "storage failure"}),
        dir.path(), None, None, &output_file, "blocked", OutputLimits::default(),
    ).await;
    assert_eq!("error", result.status);
    assert!(!dir.path().join("should-not-exist").exists());
    let outputs = dir.path().join("outputs");
    std::fs::create_dir(&outputs).unwrap();
    std::fs::write(outputs.join("same.output"), "keep existing evidence").unwrap();
    let result = run_command_with_output(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "touch should-not-exist", "timeout": 5, "brief": "existing id"}),
        dir.path(), None, None, &outputs, "same", OutputLimits::default(),
    ).await;
    assert_eq!("error", result.status);
    assert!(!dir.path().join("should-not-exist").exists());
    assert_eq!(
        "keep existing evidence",
        std::fs::read_to_string(outputs.join("same.output")).unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_stored_cancellation_permit_prevents_command_spawn() {
    let dir = workspace();
    let cancel = Arc::new(Notify::new());
    cancel.notify_one();
    let result = run_command(
        &serde_json::json!({"shell": shell_name(), "cwd": ".", "command": "touch should-not-exist", "timeout": 5, "brief": "cancel before spawn"}),
        dir.path(), None, Some(cancel),
    ).await;
    assert_eq!("cancelled", result.status);
    assert!(!dir.path().join("should-not-exist").exists());
    assert_eq!(0, result.structured.unwrap()["stdout_bytes"]);
}
