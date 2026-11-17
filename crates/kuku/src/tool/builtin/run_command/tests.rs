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

use super::super::test_helpers::workspace;
use super::{blocked_command_reason, run_command, RUN_COMMAND_MAX_CHARS};
#[cfg(unix)]
use super::{
    read_output, run_command_with_output, CommandEvent, OutputLimits, OutputRead, OutputStream,
};

#[cfg(unix)]
fn stdout_command() -> &'static str {
    "printf 'hello\nworld\n'"
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

#[cfg(unix)]
fn noisy_timeout_command() -> String {
    format!(
        "printf '{}'; sleep 2",
        "x".repeat(RUN_COMMAND_MAX_CHARS + 100)
    )
}

#[cfg(windows)]
fn noisy_timeout_command() -> String {
    // Pre-encoded: `'x' * 80100; Start-Sleep -Seconds 3` as UTF-16LE base64.
    // Using -EncodedCommand avoids cmd.exe mangling special characters (|, >, etc.).
    let b64 = "JwB4ACcAIAAqACAAOAAwADEAMAAwADsAIABTAHQAYQByAHQALQBTAGwAZQBlAHAAIAAtAFMAZQBjAG8AbgBkAHMAIAAzAA==";
    format!("powershell -EncodedCommand {b64}")
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_captures_stdout_from_workspace() {
    let dir = workspace();
    std::fs::write(dir.path().join("cwd-marker.txt"), "workspace marker\n").unwrap();
    let result = run_command(
        &serde_json::json!({"command": read_marker_command(), "timeout": 5, "brief": "read marker"}),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(result.status, "ok");
    assert!(result.summary.contains("command exited 0"));
    assert!(result.model_content.contains("stdout:"));
    assert!(result.model_content.contains("workspace marker"));
    let structured = result.structured.unwrap();
    assert_eq!(structured["kind"], "command_result");
    assert_eq!(structured["exit_code"], 0);
    assert_eq!(structured["stdout_lines"], 1);
    assert_eq!(structured["stderr_lines"], 0);
    assert_eq!(structured["timed_out"], false);
    assert_eq!(structured["line_numbered"], false);
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_treats_nonzero_exit_as_command_evidence() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"command": stderr_nonzero_command(), "timeout": 5, "brief": "test stderr"}),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(result.status, "ok");
    assert!(result.summary.contains("command exited 7"));
    assert!(result.model_content.contains("stderr:"));
    assert!(result.model_content.contains("bad"));
    assert_eq!(result.structured.as_ref().unwrap()["exit_code"], 7);
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_reports_timeout_as_tool_error() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"command": noisy_timeout_command(), "timeout": 1, "brief": "test timeout"}),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(result.status, "error");
    assert!(result.summary.contains("command timed out after 1s"));
    assert_eq!(
        result.structured.as_ref().unwrap()["kind"],
        "command_result"
    );
    assert_eq!(result.structured.as_ref().unwrap()["timed_out"], true);
    assert!(result.truncated);
    assert!(result.model_content.len() <= RUN_COMMAND_MAX_CHARS);
    assert!(result.model_content.contains("Command output truncated"));
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
        .contains("run_command requires command"));

    let empty = run_command(
        &serde_json::json!({"command": "   ", "timeout": 1, "brief": "test"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(empty.status, "error");
    assert!(empty.model_content.contains("command must not be empty"));

    let zero = run_command(
        &serde_json::json!({"command": stdout_command(), "timeout": 0, "brief": "test"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(zero.status, "error");
    assert!(zero.model_content.contains("timeout must be >= 1"));

    let over_cap = run_command(
        &serde_json::json!({"command": stdout_command(), "timeout": 601, "brief": "test"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(over_cap.status, "error");
    assert!(over_cap.model_content.contains("timeout must be <= 600"));

    let dangerous = run_command(
        &serde_json::json!({"command": "git reset --hard HEAD", "timeout": 1, "brief": "test danger"}),
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
    assert_eq!(blocked_command_reason("git push origin main"), None);
    assert_eq!(blocked_command_reason("gh pr create --fill"), None);
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
        assert!(
            blocked_command_reason(command).is_some(),
            "{command} should remain hard guarded"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn run_command_rejects_missing_brief() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"command": "echo hi", "timeout": 1}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!(result.status, "error");
    assert!(result.model_content.contains("run_command requires brief"));
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
            &serde_json::json!({"command": cmd, "timeout": 5, "brief": "stream test"}),
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
            &serde_json::json!({"command": cmd, "timeout": 5, "brief": "stderr stream"}),
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
            &serde_json::json!({"command": "echo hello", "timeout": 5, "brief": "fast output"}),
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
        &serde_json::json!({"command": "true", "timeout": 5, "brief": "no output"}),
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
            &serde_json::json!({"command": cmd, "timeout": 5, "brief": "binary output"}),
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
        &serde_json::json!({"command": "head -c 2097152 /dev/zero | tr '\\0' x", "timeout": 4, "brief": "large output"}),
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
async fn build_test_summary_survives_the_capture_quota_and_reopen() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    let result = run_command_with_output(
        &serde_json::json!({"command": "head -c 131072 /dev/zero | tr '\\0' x; printf '\\ntest result: ok. 7 passed; 0 failed; 0 ignored\\n'", "timeout": 5, "brief": "test summary"}),
        dir.path(), None, None, &outputs, "summary", OutputLimits { max_output_bytes: 8192, max_preview_bytes: 1024 },
    ).await;
    assert_eq!("ok", result.status);
    assert!(result.model_content.contains("7 passed"));
    assert!(result.model_content.contains("Capture quota reached"));
    assert_eq!(true, result.structured.as_ref().unwrap()["output_filtered"]);
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
        vec!["test result: ok. 7 passed; 0 failed; 0 ignored".to_string()],
        reopened.summary
    );
}

#[cfg(unix)]
#[tokio::test]
async fn command_persists_the_observed_stdout_stderr_sequence() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    let result = run_command_with_output(
        &serde_json::json!({"command": "printf first; sleep 0.05; printf second >&2; sleep 0.05; printf third", "timeout": 5, "brief": "ordered streams"}),
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
#[tokio::test]
async fn cancelling_a_command_stops_its_descendant_process() {
    let dir = workspace();
    let marker = dir.path().join("child.pid");
    let cancel = Arc::new(Notify::new());
    let worker_cancel = cancel.clone();
    let worker_dir = dir.path().to_path_buf();
    let worker = tokio::spawn(async move {
        run_command(&serde_json::json!({"command": "sleep 30 & echo $! > child.pid; wait", "timeout": 20, "brief": "cancel tree"}), &worker_dir, None, Some(worker_cancel)).await
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn timeout_stops_descendants_and_dropping_the_command_future_does_too() {
    let dir = workspace();
    let worker_dir = dir.path().to_path_buf();
    let worker = tokio::spawn(async move {
        run_command(&serde_json::json!({"command": "sleep 30 & echo $! > timeout.pid; wait", "timeout": 1, "brief": "timeout tree"}), &worker_dir, None, None).await
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
        run_command(&serde_json::json!({"command": "sleep 30 & echo $! > dropped.pid; wait", "timeout": 20, "brief": "drop tree"}), &worker_dir, None, None).await
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
        run_command(&serde_json::json!({"command": "setsid sh -c 'echo $$ > detached.pid; sleep 30' & while [ ! -f detached.pid ]; do sleep 0.01; done", "timeout": 5, "brief": "bounded pipe cleanup"}), &worker_dir, None, None).await
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
        &serde_json::json!({"command": "touch should-not-exist", "timeout": 5, "brief": "storage failure"}),
        dir.path(), None, None, &output_file, "blocked", OutputLimits::default(),
    ).await;
    assert_eq!("error", result.status);
    assert!(!dir.path().join("should-not-exist").exists());
    let outputs = dir.path().join("outputs");
    std::fs::create_dir(&outputs).unwrap();
    std::fs::write(outputs.join("same.output"), "keep existing evidence").unwrap();
    let result = run_command_with_output(
        &serde_json::json!({"command": "touch should-not-exist", "timeout": 5, "brief": "existing id"}),
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
        &serde_json::json!({"command": "touch should-not-exist", "timeout": 5, "brief": "cancel before spawn"}),
        dir.path(), None, Some(cancel),
    ).await;
    assert_eq!("cancelled", result.status);
    assert!(!dir.path().join("should-not-exist").exists());
    assert_eq!(0, result.structured.unwrap()["stdout_bytes"]);
}
